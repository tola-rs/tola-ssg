//! Read-only compiler diagnostics for one editor revision, even when the site does not compile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::compiler::TypstHost;
use crate::config::ResolvedSiteConfig;
use crate::content::ContentUnit;
use tola_diagnostic::Diagnostic;

/// Derivation generations the process-wide compiler cache retains across one eviction.
const RETAINED_DERIVATION_GENERATIONS: usize = 1;

/// Compiler resources for read-only checks of successive editor revisions.
pub struct SourceDiagnosticSession {
    config: Arc<ResolvedSiteConfig>,
    resources: crate::resources::BuildResources,
    environment: SourceEnvironment,
    host: Option<TypstHost>,
    icons: Option<crate::icon::IconInventory>,
    analysis: Option<CheckAnalysisEvidence>,
    reused_analysis: bool,
    frozen_modules: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceEnvironment {
    Site,
    Documents,
}

/// Source analysis and the exact inputs it consumed.
struct CheckAnalysisEvidence {
    cache: crate::compiler::analysis::SourceAnalysisCache,
    dependencies: crate::compiler::CompilationDependencies,
    /// The unsaved sources the analysis was derived from, by path.
    unsaved: BTreeMap<PathBuf, Arc<str>>,
}

/// Diagnostics and, when the check reached a world, the prepared compiler revision.
pub struct SourceRevision {
    diagnostics: Vec<Diagnostic>,
    checked: Option<SourceCompilation>,
    accessed: CheckReads,
}

#[derive(Default)]
struct CheckReads {
    disk_reads: Vec<PathBuf>,
    package_checks: Vec<tola_typst::PackageCheck>,
    file_reads: Vec<tola_typst::FileRead>,
}

impl CheckReads {
    fn record(&mut self, accessed: &tola_typst::AccessedDeps) {
        self.file_reads.extend(accessed.reads.iter().cloned());
        self.disk_reads.extend(
            accessed
                .disk_reads
                .iter()
                .map(|read| read.as_path().to_path_buf()),
        );
        self.package_checks
            .extend(accessed.package_checks.iter().cloned());
    }
}

/// Why a check stopped before it could resolve its configuration.
enum CheckStop {
    Cancelled(BuildCancelled),
    Revision(Box<SourceRevision>),
}

impl SourceRevision {
    /// Successful read-time observations from source analysis and compilation, including
    /// observations retained before a failed check. Different digests remain distinct.
    pub fn file_reads(&self) -> &[tola_typst::FileRead] {
        &self.accessed.file_reads
    }

    /// Physical read attempts made by this check, including failed input reads.
    pub fn disk_reads(&self) -> &[PathBuf] {
        &self.accessed.disk_reads
    }

    /// Every package candidate checked before this revision completed or failed.
    pub fn package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.accessed.package_checks
    }

    /// All diagnostics from this source revision.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consume the revision, retaining its complete diagnostics.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// The checked site, absent when the check failed before it resolved a world.
    pub fn checked(&self) -> Option<&SourceCompilation> {
        self.checked.as_ref()
    }

    /// Take the compilation this revision checked, absent when it reached no world.
    pub fn into_checked(self) -> Option<SourceCompilation> {
        self.checked
    }

    /// Whether this revision compiled: the site's root program, or the one source a check
    /// compiled on its own.
    pub fn compiled(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == tola_diagnostic::Severity::Error)
            && self
                .checked
                .as_ref()
                .is_some_and(|checked| checked.evaluation.is_some())
    }
}

/// A checked site: the resolved world, and the Bundle when the root program compiled.
pub struct SourceCompilation {
    world: Arc<tola_typst::TypstWorld>,
    /// The root program this world attempted, absent for independent source checks.
    site_entry: Option<typst::syntax::FileId>,
    /// What compiled, absent when nothing did.
    evaluation: Option<Evaluation>,
}

/// What one check compiled.
enum Evaluation {
    /// The site's root Bundle.
    Site(Arc<tola_typst::BundleCompilation>),
    /// One source of the site, compiled without the root program, with the reads its
    /// compilation made.
    Source {
        document: Box<tola_typst::HtmlDocument>,
        file_reads: Vec<tola_typst::FileRead>,
    },
}

enum SourceCheckInput<'a> {
    Path(&'a str),
    File(typst::syntax::FileId),
}

impl SourceCompilation {
    /// The immutable world containing the inputs this check resolved.
    pub fn world(&self) -> &tola_typst::TypstWorld {
        &self.world
    }

    /// The actual root program of this check, including a package driver.
    /// A standalone fallback never declares site-wide metadata, even if it compiles.
    pub fn site_entry(&self) -> Option<typst::syntax::FileId> {
        self.site_entry
    }

    /// The realized Bundle, before file export or publication, absent when the root program did
    /// not compile.
    pub fn bundle(&self) -> Option<&tola_typst::BundleCompilation> {
        match &self.evaluation {
            Some(Evaluation::Site(bundle)) => Some(bundle),
            _ => None,
        }
    }

    /// Typst's introspection of what compiled, absent when nothing did.
    pub fn introspector(&self) -> Option<&dyn tola_typst::typst::introspection::Introspector> {
        match &self.evaluation {
            Some(Evaluation::Site(bundle)) => Some(bundle.introspector()),
            Some(Evaluation::Source { document, .. }) => Some(document.introspector()),
            None => None,
        }
    }

    /// The reads the compilation made, empty when nothing compiled.
    pub fn file_reads(&self) -> &[tola_typst::FileRead] {
        match &self.evaluation {
            Some(Evaluation::Site(bundle)) => bundle.file_reads(),
            Some(Evaluation::Source { file_reads, .. }) => file_reads,
            None => &[],
        }
    }

    /// The addressable HTML documents this compilation realized, in document order.
    pub fn realized_documents(
        &self,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<Vec<RealizedDocument>> {
        let Some(Evaluation::Site(bundle)) = &self.evaluation else {
            return Ok(Vec::new());
        };
        let cancellation = cancellation.bundle_cancellation();
        let mut documents = Vec::new();
        for document in bundle.documents() {
            cancellation.ensure_active()?;
            if document.html_inventory(&cancellation)?.is_none() {
                continue;
            }
            let output = crate::compiler::documents::output_path_for_virtual(document.path())?;
            documents.push(RealizedDocument {
                permalink: tola_address::route_for_output(&output),
                output,
                sources: document.source_ids(),
                rendered_sources: document.rendered_source_ids(&cancellation)?,
            });
        }
        Ok(documents)
    }
}

/// One document a compilation realized, with the address it is served at.
#[derive(Debug)]
pub struct RealizedDocument {
    /// The logical output path the document is written to.
    pub output: tola_address::OutputPath,
    /// The site-relative route the document is served at.
    pub permalink: tola_address::UrlPath,
    /// Original body identities: provenance, not ownership.
    pub sources: Vec<typst::syntax::FileId>,
    /// Additional identities from realized HTML; these are contributions, not primary ownership.
    pub rendered_sources: Vec<typst::syntax::FileId>,
}

impl CheckAnalysisEvidence {
    fn decide(
        &self,
        content: &[ContentUnit],
        bindings: &crate::package::SiteBindings,
        unsaved: &BTreeMap<PathBuf, Arc<str>>,
        boundary: &tola_typst::SourceBoundary,
        cancellation: &BuildCancellation,
    ) -> Result<Option<crate::compiler::RebuildDecision>, BuildCancelled> {
        let reason = if !self.cache.matches_content_identity(content) {
            Some("the site's content membership changed")
        } else if !self.cache.matches_package_bindings(bindings) {
            Some("the site's packages or asset addresses changed")
        } else {
            None
        };
        if let Some(reason) = reason {
            tracing::debug!(reason, "check re-derives its source analysis");
            return Ok(None);
        }

        let mut changed = self
            .dependencies
            .changed_physical_paths(boundary, cancellation)?;
        changed.extend(self.dependencies.changed_package_paths(cancellation)?);
        changed.extend(
            self.unsaved
                .keys()
                .chain(unsaved.keys())
                .filter(|path| self.unsaved.get(*path) != unsaved.get(*path))
                .cloned(),
        );
        changed.sort_unstable();
        changed.dedup();
        Ok(Some(crate::compiler::RebuildDecision::for_observed_paths(
            &self.dependencies,
            &changed,
        )))
    }

    /// Collect what one evaluated revision read, in the shape the build path publishes it.
    fn of(
        evaluated: &crate::compiler::bundle::EvaluatedSiteProgram,
        compilation: &tola_typst::BundleCompilation,
        config: &ResolvedSiteConfig,
        content: &[ContentUnit],
        unsaved: &BTreeMap<PathBuf, Arc<str>>,
        host: &TypstHost,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<Self> {
        cancellation.ensure_active()?;
        let reader_evidence = evaluated.dependency_readers(compilation);
        let package_checks = evaluated.package_checks(compilation).cloned();
        let dependencies = crate::compiler::CompilationDependencies::new(
            config.get_root(),
            content.iter().map(|unit| unit.source.clone()),
            std::iter::once(config.build.entry.clone()),
            reader_evidence,
            package_checks,
            host,
            None,
        )?;
        Ok(Self {
            cache: evaluated.source_analysis().clone(),
            dependencies,
            unsaved: unsaved.clone(),
        })
    }
}

/// What one check collected while it evaluated a revision, including what a failed attempt read.
#[derive(Default)]
struct RevisionEvaluation {
    world: Option<Arc<tola_typst::TypstWorld>>,
    reads: crate::compiler::BuildInputs,
    file_reads: Vec<tola_typst::FileRead>,
    warnings: tola_typst::Diagnostics,
    declaration_warnings: Vec<Diagnostic>,
    reused_site_program: bool,
}

impl RevisionEvaluation {
    /// Keep the world a failed stage resolved, then return its failure.
    fn keep_failed_world(
        &mut self,
        world: Option<Arc<tola_typst::TypstWorld>>,
        error: anyhow::Error,
    ) -> anyhow::Error {
        self.world.clone_from(&world);
        error
    }
}

impl SourceDiagnosticSession {
    /// Create a session for one resolved configuration.
    pub fn new(config: Arc<ResolvedSiteConfig>) -> Self {
        Self::with_resources(config, crate::resources::BuildResources::default())
    }

    pub fn with_resources(
        config: Arc<ResolvedSiteConfig>,
        resources: crate::resources::BuildResources,
    ) -> Self {
        Self {
            config,
            resources,
            environment: SourceEnvironment::Site,
            host: None,
            icons: None,
            analysis: None,
            reused_analysis: false,
            frozen_modules: false,
        }
    }

    /// A one-shot query already selected its entry from these prepared module inputs.
    /// Its caller owns the manifest freshness checks around inspection.
    pub(crate) fn with_prepared_modules(
        config: Arc<ResolvedSiteConfig>,
        resources: crate::resources::BuildResources,
    ) -> Self {
        Self {
            frozen_modules: true,
            ..Self::with_resources(config, resources)
        }
    }

    /// Create a session for independent documents without a site's content inventory.
    pub fn for_documents(
        config: Arc<ResolvedSiteConfig>,
        resources: crate::resources::BuildResources,
    ) -> Self {
        Self {
            environment: SourceEnvironment::Documents,
            ..Self::with_resources(config, resources)
        }
    }

    pub fn resources(&self) -> &crate::BuildResources {
        &self.resources
    }

    /// Whether every content source reused its previous analysis.
    pub fn reused_source_analysis(&self) -> bool {
        self.reused_analysis
    }

    /// Release the parsed sources one session stopped reading; a live world keeps its slots.
    pub fn evict_stale_file_cache_entries(&self) {
        if let Some(host) = &self.host {
            host.evict_stale_file_cache_entries(TypstHost::RETAINED_FILE_CACHE_EPOCHS);
        }
    }

    /// Check one immutable set of unsaved sources without changing source files.
    pub fn check(
        &mut self,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<Diagnostic>, BuildCancelled> {
        self.inspect(overlays, cancellation)
            .map(SourceRevision::into_diagnostics)
    }

    /// The host with the unsaved sources, the discovered content, and the bindings a check
    /// resolves packages through.
    fn prepared(
        &mut self,
        config: &ResolvedSiteConfig,
        overrides: &crate::filesystem::SourceOverrides,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<(
        TypstHost,
        Vec<crate::content::ContentUnit>,
        crate::package::SiteBindings,
    )> {
        let boundary = self.resources.source_boundary(config);
        if self.environment == SourceEnvironment::Site {
            boundary.check(&config.build.content_dir)?;
        }
        for path in overrides
            .paths()
            .chain(config.assets.tree_sources())
            .chain(config.assets.file_sources())
        {
            boundary.check(path)?;
        }
        let reusable = self
            .host
            .as_ref()
            .filter(|host| host.matches(config, &self.resources))
            .map(|host| host.font_inventory_is_fresh(cancellation))
            .transpose()?
            .unwrap_or(false);
        if !reusable {
            let host = TypstHost::for_config_with_resources(config, &self.resources, cancellation)
                .map_err(tola_typst::CompileError::from)?;
            cancellation.ensure_active()?;
            self.host = Some(host);
            self.analysis = None;
        }
        let host = self
            .host
            .as_ref()
            .expect("compiler resources are ready")
            .with_source_overrides(overrides)?;
        let content = match self.environment {
            SourceEnvironment::Site => {
                let content = crate::content::discover_content_units_for_config_with_cancellation(
                    config,
                    cancellation,
                )?;
                crate::content::content_units_with_sources(
                    config,
                    &content,
                    overrides.paths().map(PathBuf::from),
                    cancellation,
                )?
            }
            SourceEnvironment::Documents => Vec::new(),
        };
        let packages = crate::package::prepare_package_inputs(
            config,
            &self.resources,
            cancellation,
            &mut self.icons,
            overrides,
        )?;
        let host = host
            .with_icons(packages.icons.collections())
            .with_local_packages(Arc::clone(&config.module_inputs.packages));
        let bindings = crate::package::SiteBindings::from_config(
            config,
            crate::asset::AssetUrls::for_check(config, cancellation)?,
        )
        .prepare_modules(config, &host, cancellation)?;
        Ok((host, content, bindings))
    }

    /// The configuration a check runs under, or why it stopped.
    fn check_configuration(
        &mut self,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<ResolvedSiteConfig>, CheckStop> {
        cancellation.ensure_active().map_err(CheckStop::Cancelled)?;
        if self.frozen_modules {
            return Ok(Arc::clone(&self.config));
        }
        crate::modules::prepared::configure(&self.config, self.resources.input_scope())
            .map(Arc::new)
            .map_err(|error| {
                CheckStop::Revision(Box::new(SourceRevision {
                    diagnostics: source_failure_diagnostics(&error, self.config.get_root()),
                    checked: None,
                    accessed: CheckReads::default(),
                }))
            })
    }

    /// Check one source on its own, with the diagnostics it produced.
    pub fn inspect_source(
        &mut self,
        relative: &str,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        self.inspect_source_input(SourceCheckInput::Path(relative), overlays, cancellation)
    }

    /// Check a complete source identity, preserving a local package's own import root.
    pub fn inspect_source_file(
        &mut self,
        source: typst::syntax::FileId,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        self.inspect_source_input(SourceCheckInput::File(source), overlays, cancellation)
    }

    fn inspect_source_input(
        &mut self,
        source: SourceCheckInput<'_>,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        let configuration = match self.check_configuration(cancellation) {
            Ok(configuration) => configuration,
            Err(CheckStop::Cancelled(error)) => return Err(error),
            Err(CheckStop::Revision(revision)) => return Ok(*revision),
        };
        let config = configuration.as_ref();
        let mut diagnostics = Vec::new();
        let mut accessed = CheckReads::default();
        let prepared =
            (|| -> anyhow::Result<Option<(Arc<tola_typst::TypstWorld>, typst::syntax::FileId)>> {
                let overrides = crate::filesystem::SourceOverrides::new(overlays, cancellation)?;
                let (host, content, bindings) = self.prepared(config, &overrides, cancellation)?;
                let sources = crate::content::SourceSet::without_metadata(&content, config)?;
                let library = bindings.library(sources.inputs().to_source_records());
                let files = host.file_resolver();
                let main = match source {
                    SourceCheckInput::File(id) => id,
                    SourceCheckInput::Path(relative) => {
                        let path = tola_typst::normalize_path(&config.get_root().join(relative));
                        host.source_boundary().check(&path)?;
                        let main = tola_typst::file_id_from_path(&path, config.get_root())
                            .or_else(|| {
                                let ids = files.local_package_ids(&path).ok()?;
                                match ids.as_slice() {
                                    [id] => Some(*id),
                                    _ => None,
                                }
                            })
                            .or_else(|| {
                                crate::filesystem::package_document_id(
                                    &path,
                                    Some(config.package_locations()),
                                )
                            });
                        let Some(main) = main else { return Ok(None) };
                        main
                    }
                };
                let explicit = if matches!(main.root(), typst::syntax::VirtualRoot::Project) {
                    vec![config.get_root().join(main.vpath().get_without_slash())]
                } else {
                    Vec::new()
                };
                let loaded = tola_typst::ExplicitSources::build_with_files_cancellable(
                    &explicit,
                    config.get_root(),
                    files,
                    &cancellation.bundle_cancellation(),
                )
                .map_err(|failure| {
                    let (error, observed) = failure.into_error_and_accessed();
                    accessed.record(&observed);
                    anyhow::Error::new(tola_typst::CompileError::from(error))
                })?;
                let (sources, observed) = loaded.into_sources_and_accessed();
                accessed.record(&observed);
                let candidate_files = host.candidate_files(Arc::new(sources));
                let world = Arc::new(host.world_with_id(
                    config.get_root(),
                    main,
                    &library,
                    candidate_files,
                    &cancellation.bundle_cancellation(),
                )?);
                Ok(Some((world, main)))
            })();
        cancellation.ensure_active()?;
        let prepared = match prepared {
            Ok(Some(prepared)) => Some(prepared),
            Ok(None) => None,
            Err(error) => {
                diagnostics.extend(source_failure_diagnostics(&error, config.get_root()));
                None
            }
        };
        let Some((world, main)) = prepared else {
            return Ok(SourceRevision {
                diagnostics,
                checked: None,
                accessed,
            });
        };
        let mut warnings = tola_typst::Diagnostics::new();
        // A source that does not compile on its own still resolves what it imports, so the world
        // is what an editor answers from; the evaluation is present only when the file compiled.
        let evaluation = match tola_typst::compile_source_with_evidence(&world, main) {
            Ok(compiled) => {
                warnings.extend_distinct(compiled.diagnostics());
                let (document, observed, _) = compiled.into_parts();
                accessed.record(&observed);
                Some(Evaluation::Source {
                    document: Box::new(document),
                    file_reads: observed.reads,
                })
            }
            Err(failure) => {
                if failure.is_cancelled() {
                    return Err(BuildCancelled);
                }
                let (observed, error) = failure.into_parts();
                accessed.record(&observed);
                diagnostics.extend(source_failure_diagnostics(
                    &anyhow::Error::new(error),
                    config.get_root(),
                ));
                None
            }
        };
        diagnostics.extend(crate::compiler::warning_diagnostics(config, &mut warnings));
        tola_diagnostic::coalesce_reports(&mut diagnostics);
        Ok(SourceRevision {
            diagnostics,
            accessed,
            checked: Some(SourceCompilation {
                world,
                site_entry: None,
                evaluation,
            }),
        })
    }

    /// Evaluate one revision against the session's held world and prepared inputs, collecting
    /// everything the caller reports even when the attempt fails.
    fn evaluate_revision(
        &mut self,
        config: &ResolvedSiteConfig,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
        evaluation: &mut RevisionEvaluation,
    ) -> anyhow::Result<SourceCompilation> {
        let overrides = crate::filesystem::SourceOverrides::new(overlays, cancellation)?;
        let unsaved = overrides
            .sources()
            .map(|(path, source)| (path.to_path_buf(), Arc::clone(&source.text)))
            .collect::<BTreeMap<_, _>>();
        let (host, content, bindings) = self.prepared(config, &overrides, cancellation)?;
        let mut prepared = None;
        let mut reuse = crate::compiler::analysis::SourceAnalysisReuse::None;
        if let Some(held) = &self.analysis
            && let Some(decision) = held.decide(
                &content,
                &bindings,
                &unsaved,
                host.source_boundary(),
                cancellation,
            )?
        {
            evaluation.reused_site_program = decision.reuses_site_program()
                && held.dependencies.virtual_reads_match(
                    |evidence| match evidence.locator() {
                        tola_typst::ReadLocator::ProvidedRoot(relative) => overrides
                            .get_physical(&crate::filesystem::normalize_existing_prefix(
                                &config.get_root().join(relative),
                            ))
                            .is_some_and(|source| source.digest == evidence.digest()),
                        _ => host.virtual_read_matches(evidence),
                    },
                    cancellation,
                )?;
            prepared = Some(
                held.cache
                    .clone()
                    .prepare_for_rebuild(&decision, config.project_entry_path().as_deref()),
            );
            reuse = crate::compiler::analysis::SourceAnalysisReuse::retained(
                prepared
                    .as_ref()
                    .expect("the reuse holds its prepared cache"),
                &held.dependencies,
            );
        }
        let evaluated = crate::compiler::bundle::evaluate(
            config,
            &host,
            &content,
            // A check renders no configured assets: declarations resolve to their mounted names.
            bindings,
            &cancellation.bundle_cancellation(),
            reuse,
            &mut evaluation.reads,
        )
        .map_err(|failure| evaluation.keep_failed_world(failure.world, failure.error))?;
        self.reused_analysis = prepared.is_some()
            && evaluated
                .source_analysis()
                .reused_dependency_readers()
                .len()
                == content.len();
        evaluation
            .warnings
            .extend_distinct(evaluated.source_analysis().diagnostics());
        evaluation
            .declaration_warnings
            .extend_from_slice(evaluated.source_analysis().declaration_warnings());
        let realized = crate::compiler::bundle::realize(
            config,
            &host,
            &cancellation.bundle_cancellation(),
            &evaluated,
            &mut evaluation.reads,
        )
        .map_err(|failure| evaluation.keep_failed_world(failure.world, failure.error))?;
        evaluation
            .warnings
            .extend_distinct(realized.compilation.diagnostics());
        for (_, reads, _) in evaluated.dependency_readers(&realized.compilation) {
            evaluation.file_reads.extend(reads);
        }
        self.analysis = Some(CheckAnalysisEvidence::of(
            &evaluated,
            &realized.compilation,
            config,
            &content,
            &unsaved,
            &host,
            cancellation,
        )?);
        Ok(SourceCompilation {
            evaluation: Some(Evaluation::Site(Arc::new(realized.compilation))),
            site_entry: (self.environment == SourceEnvironment::Site)
                .then_some(typst::World::main(realized.world.as_ref())),
            world: realized.world,
        })
    }

    /// Prepare one immutable revision for diagnostics and semantic queries.
    pub fn inspect(
        &mut self,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        let configuration = match self.check_configuration(cancellation) {
            Ok(configuration) => configuration,
            Err(CheckStop::Cancelled(error)) => return Err(error),
            Err(CheckStop::Revision(revision)) => return Ok(*revision),
        };
        let config = configuration.as_ref();
        self.reused_analysis = false;
        let mut evaluation = RevisionEvaluation::default();
        let checked = self.evaluate_revision(config, overlays, cancellation, &mut evaluation);
        // Repeated queries keep their derivations; an edit ages superseded calculations.
        if !evaluation.reused_site_program || !self.reused_analysis || checked.is_err() {
            typst::comemo::evict(RETAINED_DERIVATION_GENERATIONS);
        }
        cancellation.ensure_active()?;
        let mut diagnostics = evaluation.declaration_warnings;
        diagnostics.extend(crate::compiler::warning_diagnostics(
            config,
            &mut evaluation.warnings,
        ));
        tola_diagnostic::coalesce_reports(&mut diagnostics);
        let mut file_reads = evaluation.file_reads;
        file_reads.extend(evaluation.reads.successful_reads().cloned());
        let mut seen = std::collections::HashSet::new();
        file_reads.retain(|read| seen.insert(read.clone()));
        let (disk_reads, package_checks) = evaluation.reads.into_parts();
        let accessed = CheckReads {
            disk_reads,
            package_checks,
            file_reads,
        };
        match checked {
            Ok(compilation) => Ok(SourceRevision {
                diagnostics,
                checked: Some(compilation),
                accessed,
            }),
            Err(error) if crate::cancellation::is_cancelled(&error) => Err(BuildCancelled),
            Err(error) => {
                // A source check runs configuration, content, package, and compilation stages, so it
                // classifies exactly those producers rather than the whole publication chain.
                let classified = tola_diagnostic::attached(&error)
                    .map(<[tola_diagnostic::Diagnostic]>::to_vec)
                    .or_else(|| crate::compiler::error_diagnostics(&error, config.get_root()))
                    .or_else(|| {
                        crate::icon::error_diagnostic(&error, config.get_root())
                            .map(|diagnostic| vec![diagnostic])
                    })
                    .unwrap_or_else(|| {
                        vec![tola_diagnostic::fallback(
                            tola_diagnostic::codes::check::SOURCE,
                            &error,
                        )]
                    });
                diagnostics.extend(classified);
                tola_diagnostic::coalesce_reports(&mut diagnostics);
                Ok(SourceRevision {
                    diagnostics,
                    accessed,
                    checked: evaluation.world.map(|world| SourceCompilation {
                        site_entry: (self.environment == SourceEnvironment::Site)
                            .then_some(typst::World::main(world.as_ref())),
                        world,
                        evaluation: None,
                    }),
                })
            }
        }
    }
}

/// Classify one source-lane failure into diagnostics the site author reads.
fn source_failure_diagnostics(
    error: &anyhow::Error,
    root: &Path,
) -> Vec<tola_diagnostic::Diagnostic> {
    tola_diagnostic::attached(error)
        .map(<[tola_diagnostic::Diagnostic]>::to_vec)
        .or_else(|| crate::compiler::error_diagnostics(error, root))
        .unwrap_or_else(|| {
            vec![tola_diagnostic::fallback(
                tola_diagnostic::codes::check::SOURCE,
                error,
            )]
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tola_diagnostic::Severity;

    fn site() -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::TempDir::new().unwrap();
        fs::create_dir(directory.path().join("content")).unwrap();
        fs::write(
            directory.path().join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": join-route, output-of
#for source in all-sources() {
  document(output-of(join-route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();
        let config = Arc::new(crate::config::tests::load_test_config(directory.path(), ""));
        (directory, config)
    }

    fn documents_workspace() -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::TempDir::new().unwrap();
        fs::write(
            directory.path().join("page.typ"),
            "#let extra = read(\"/data.txt\")\nBody: #extra\n",
        )
        .unwrap();
        fs::write(directory.path().join("data.txt"), "payload\n").unwrap();
        let source =
            crate::config::ConfigSource::defaults(directory.path(), crate::InputScope::Online)
                .unwrap();
        let config = Arc::new(
            crate::config::loading::resolve_parsed_site_config(
                source.decode::<crate::config::SiteConfigSchema>().unwrap(),
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        );
        (directory, config)
    }

    /// The documents a revision realizes, with the routes they are served at.
    fn documents(
        revision: &SourceRevision,
        cancellation: &BuildCancellation,
    ) -> Vec<(String, String)> {
        revision
            .checked()
            .map(|checked| {
                checked
                    .realized_documents(cancellation)
                    .unwrap()
                    .into_iter()
                    .map(|document| (document.output.to_string(), document.permalink.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A check reports Typst's failure at the author's own source line, and never writes output.
    #[test]
    fn source_errors_report_their_location() {
        let (_directory, config) = site();
        fs::write(
            config.build.content_dir.join("post.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: panic(\"is also a published asset URL\")))\n",
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(config.clone());
        let diagnostics = session
            .check(Vec::new(), &BuildCancellation::default())
            .unwrap();

        let errors = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, tola_diagnostic::codes::typst::COMPILE);
        assert_eq!(
            errors[0].location.as_ref().unwrap().path,
            "content/post.typ"
        );
        assert_eq!(errors[0].location.as_ref().unwrap().line, Some(2));
        assert!(!config.build.publish_dir.exists());
    }

    /// A pure check refuses text for a path outside the site root, and writes nothing there.
    #[cfg(unix)]
    #[test]
    fn pure_checks_reject_unsaved_host_sources() {
        let (_directory, config) = site();
        let outside = tempfile::TempDir::new().unwrap();
        let alias = config.get_root().join("linked");
        std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
        let mut session = SourceDiagnosticSession::with_resources(
            config,
            crate::BuildResources::new().with_input_scope(crate::InputScope::Pure),
        );
        let diagnostics = session
            .check(
                vec![(alias.join("new.typ"), Arc::from("unsaved host text"))],
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        assert!(!outside.path().join("new.typ").exists());
    }

    /// An unsaved revision is checked from memory: the disk file it shadows keeps its bytes, and
    /// no output is published.
    #[test]
    fn unsaved_overlays_leave_disk_unchanged() {
        for (name, existing_disk, source, overlay, needle) in [
            (
                "metadata and body share the editor revision",
                Some("#missing_on_disk"),
                "document.typ",
                r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Draft], pin: true))
#import "@tola/document:0.0.0": current-document
#context [#current-document().route]
= Unsaved body"#,
                None,
            ),
            (
                "new content is discovered and reports its own error",
                None,
                "new.typ",
                "#unsaved_error",
                Some("unsaved_error"),
            ),
        ] {
            let (_directory, config) = site();
            let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
            let source = config.build.content_dir.join(source);
            if let Some(disk) = existing_disk {
                fs::write(&source, disk).unwrap();
            }
            let diagnostics = session
                .check(
                    vec![(source.clone(), Arc::from(overlay))],
                    &BuildCancellation::new(),
                )
                .unwrap();

            match needle {
                Some(needle) => assert!(
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic.message.contains(needle)
                            && diagnostic
                                .location
                                .as_ref()
                                .is_some_and(|location| location.path == "content/new.typ")
                    }),
                    "{name}: {diagnostics:#?}"
                ),
                None => assert!(
                    diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.severity != Severity::Error),
                    "{name}: {diagnostics:#?}"
                ),
            }
            assert_eq!(
                fs::read_to_string(&source).ok().as_deref(),
                existing_disk,
                "{name} must leave the disk file unchanged"
            );
            assert!(
                !config.build.publish_dir.exists(),
                "{name} must not write output"
            );
        }
    }

    /// A source of a site whose own program does not compile checks on its own, because that is
    /// what the editor answers about the file the author is editing from.
    #[test]
    fn source_checks_alone_site_fails() {
        let (_directory, config) = site();
        fs::write(
            config.build.content_dir.join("document.typ"),
            "#let brand = 1\n",
        )
        .unwrap();
        fs::write(
            config.build.content_dir.join("broken.typ"),
            "#let nothing = absent\n",
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let cancellation = BuildCancellation::new();
        let site_revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            site_revision
                .checked()
                .is_some_and(|checked| checked.bundle().is_none()),
            "the site compiles no Bundle"
        );

        let revision = session
            .inspect_source("content/document.typ", Vec::new(), &cancellation)
            .unwrap();
        let checked = revision.checked().expect("a source checked on its own");
        assert!(checked.bundle().is_none(), "no root Bundle compiled");
        assert!(checked.introspector().is_some(), "the source compiled");
        assert!(
            checked
                .realized_documents(&cancellation)
                .unwrap()
                .is_empty(),
            "a source alone realizes no page"
        );
    }

    /// A compiled document records what it read from disk, which is the evidence an editor
    /// filters file-change notifications by.
    #[test]
    fn compiled_document_records_its_reads() {
        let (directory, config) = documents_workspace();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::for_documents(
            config.clone(),
            crate::BuildResources::default(),
        );
        let revision = session
            .inspect_source("page.typ", Vec::new(), &cancellation)
            .unwrap();
        assert!(revision.compiled(), "{:#?}", revision.diagnostics());
        let data = fs::canonicalize(directory.path().join("data.txt")).unwrap();
        let reads = revision
            .checked()
            .expect("the document resolved a world")
            .file_reads()
            .iter()
            .filter_map(|read| match read.origin() {
                tola_typst::ReadOrigin::Disk(path) => Some(path.as_path().to_path_buf()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(reads.contains(&data), "{reads:?}");
    }

    /// A failed compilation establishes no read evidence.
    #[test]
    fn failed_compilation_records_no_reads() {
        let (directory, config) = documents_workspace();
        fs::write(
            directory.path().join("page.typ"),
            "#let extra = read(\"/data.txt\")\n#absent_name\n",
        )
        .unwrap();
        let cancellation = BuildCancellation::new();
        let mut session =
            SourceDiagnosticSession::for_documents(config, crate::BuildResources::default());
        let revision = session
            .inspect_source("page.typ", Vec::new(), &cancellation)
            .unwrap();
        assert!(!revision.compiled(), "{:#?}", revision.diagnostics());
        assert!(
            revision
                .checked()
                .expect("the document resolved a world")
                .file_reads()
                .is_empty(),
            "a failed compilation establishes no read evidence"
        );
    }

    /// A site session requires the content root it is configured with, and says so.
    #[test]
    fn site_sessions_require_content_roots() {
        let (_directory, config) = documents_workspace();
        let mut session = SourceDiagnosticSession::new(config);
        let cancellation = BuildCancellation::new();
        for revision in [
            session.inspect(Vec::new(), &cancellation).unwrap(),
            session
                .inspect_source("page.typ", Vec::new(), &cancellation)
                .unwrap(),
        ] {
            assert!(!revision.compiled());
            assert!(
                revision
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("content root `content`")),
                "{:?}",
                revision.diagnostics()
            );
        }
    }

    /// A failed revision keeps the world its own sources resolve against, so an editor still
    /// answers about the file it is being asked about.
    #[test]
    fn failed_revision_keeps_its_own_world() {
        use typst::World;

        let (_directory, config) = site();
        let source = config.build.content_dir.join("document.typ");
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let cancellation = BuildCancellation::new();
        let success = session
            .inspect(
                vec![(source.clone(), Arc::from("Unsaved body"))],
                &cancellation,
            )
            .unwrap();
        let compilation = success.checked().unwrap();
        assert!(
            compilation
                .bundle()
                .expect("a compiled Bundle")
                .outputs()
                .any(|(path, _)| path.get_with_slash() == "/document/index.html")
        );
        let id = tola_typst::file_id_from_path(&source, config.get_root()).unwrap();
        assert_eq!(
            compilation.world().source(id).unwrap().text(),
            "Unsaved body"
        );

        let failure = session
            .inspect(
                vec![(source.clone(), Arc::from("#missing_value"))],
                &cancellation,
            )
            .unwrap();
        let checked = failure
            .checked()
            .expect("a failed revision resolves the world its sources answer from");
        assert!(checked.bundle().is_none(), "the site did not compile");
        assert_eq!(checked.world().source(id).unwrap().text(), "#missing_value");
        assert!(
            failure
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        // The earlier revision keeps the world it was checked against.
        assert_eq!(
            compilation.world().source(id).unwrap().text(),
            "Unsaved body"
        );
        assert!(!source.exists());
        assert!(!config.build.publish_dir.exists());
    }

    /// A source that disappears between checks stops being diagnosed, and the site compiles again.
    #[test]
    fn disappearing_source_loses_diagnostics() {
        let (_directory, config) = site();
        let broken = config.build.content_dir.join("broken.typ");
        fs::write(&broken, "#include \"absent.typ\"\n").unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let failed = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            failed
                .checked()
                .is_some_and(|checked| checked.bundle().is_none()),
            "a missing include fails the Bundle"
        );

        fs::remove_file(&broken).unwrap();
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            revision
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{:#?}",
            revision.diagnostics()
        );
        assert!(
            revision
                .checked()
                .is_some_and(|checked| checked.bundle().is_some()),
            "the site compiles once the source is gone"
        );
    }

    /// Reuse follows every input the source analysis read: an unchanged revision reuses, and a
    /// change to an unsaved source, a read file on disk, or the set of sources re-derives.
    #[test]
    fn reuse_follows_analysis_inputs() {
        #[derive(Clone, Copy)]
        enum Change {
            None,
            Opened,
            Edited,
            DiskDependency,
            AddedSource,
        }
        for (change, reused) in [
            (Change::None, true),
            (Change::Opened, false),
            (Change::Edited, false),
            (Change::DiskDependency, false),
            (Change::AddedSource, false),
        ] {
            let (directory, config) = site();
            let content = directory.path().join("content/post.typ");
            let dependency = directory.path().join("content/other.typ");
            fs::write(&content, "Body.\n").unwrap();
            fs::write(&dependency, "Dependency.\n").unwrap();
            let cancellation = BuildCancellation::new();
            let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
            // Every case but `Opened` starts from a revision holding an unsaved document, so the
            // reuse it is measured against rests on the same open document it repeated.
            let held = Arc::<str>::from("Unsaved body.\n");
            let first = match change {
                Change::Opened => session.inspect(Vec::new(), &cancellation).unwrap(),
                _ => session
                    .inspect(vec![(content.clone(), Arc::clone(&held))], &cancellation)
                    .unwrap(),
            };
            assert!(
                !session.reused_source_analysis(),
                "a cold revision has nothing to reuse"
            );

            let overlays = match change {
                Change::Opened => vec![(content.clone(), Arc::clone(&held))],
                Change::Edited => vec![(content.clone(), Arc::from("Edited body.\n"))],
                _ => vec![(content.clone(), Arc::clone(&held))],
            };
            match change {
                Change::DiskDependency => fs::write(&dependency, "Changed on disk.\n").unwrap(),
                Change::AddedSource => {
                    fs::write(directory.path().join("content/added.typ"), "Added.\n").unwrap()
                }
                _ => {}
            }
            let repeated = session.inspect(overlays.clone(), &cancellation).unwrap();
            assert_eq!(
                session.reused_source_analysis(),
                reused,
                "reuse must follow the inputs the analysis read"
            );

            // A re-derived check answers what a cold check answers.
            if !reused {
                let mut cold = SourceDiagnosticSession::new(Arc::clone(&config));
                let cold_revision = cold.inspect(overlays, &cancellation).unwrap();
                assert_eq!(
                    documents(&repeated, &cancellation),
                    documents(&cold_revision, &cancellation),
                );
            }
            let _ = first;
        }
    }

    /// A revision that reuses its analysis realizes the same documents at the same routes as one
    /// that does not.
    #[test]
    fn reused_revision_realizes_the_same_documents() {
        let (_directory, config) = site();
        fs::write(config.build.content_dir.join("post.typ"), "Body.\n").unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let first = session.inspect(Vec::new(), &cancellation).unwrap();
        let reused = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            session.reused_source_analysis(),
            "an unchanged revision reuses its analysis"
        );

        let mut cold = SourceDiagnosticSession::new(Arc::clone(&config));
        let fresh = cold.inspect(Vec::new(), &cancellation).unwrap();
        assert_eq!(
            documents(&first, &cancellation),
            documents(&fresh, &cancellation)
        );
        assert_eq!(
            documents(&reused, &cancellation),
            documents(&fresh, &cancellation)
        );
    }

    /// The asset URL Tola's image natives read is compared with what the site publishes there, so
    /// a declaration that shadows another one warns once at the site program and nothing else.
    ///
    /// An unambiguous URL, an undeclared one, and a tree member all stay silent: only a value two
    /// declarations disagree about is the author's problem.
    fn shadowed_path_site(
        declarations: Vec<(std::path::PathBuf, &str)>,
        written: &str,
    ) -> (tempfile::TempDir, Arc<ResolvedSiteConfig>, Vec<Diagnostic>) {
        let (directory, mut config) = site();
        let root = config.get_root().to_path_buf();
        let assets = root.join("assets");
        fs::create_dir_all(assets.join("brand")).unwrap();
        for (name, id) in [
            ("logo.svg", "logo"),
            ("plain.svg", "plain"),
            ("brand.svg", "sitting"),
            ("brand/logo.svg", "brand"),
        ] {
            fs::write(
                assets.join(name),
                format!("<svg width=\"4\" height=\"4\" id=\"{id}\"/>"),
            )
            .unwrap();
        }
        let configured = Arc::get_mut(&mut config).unwrap();
        configured.assets.files = declarations
            .into_iter()
            .map(|(source, url)| {
                crate::config::section::AssetFileDeclaration::new(
                    root.join(source),
                    crate::config::section::AssetUrl::parse(url).unwrap(),
                )
            })
            .collect();
        configured.assets.cache_busting = true;
        let entry = configured.build.entry.clone();
        fs::write(
            &entry,
            format!(
                "#import \"@tola/image:0.0.0\": image-metadata\n\
                 #document(\"index.html\")[\n  #metadata(image-metadata(\"{written}\"))\n]\n"
            ),
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let cancellation = BuildCancellation::new();
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        let diagnostics = revision.diagnostics().to_vec();
        (directory, config, diagnostics)
    }

    fn shadowed_path_warnings(diagnostics: &[Diagnostic]) -> Vec<&Diagnostic> {
        diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == tola_diagnostic::codes::reference::ASSET_URL_SHADOWED
            })
            .collect()
    }

    #[test]
    fn ambiguous_asset_urls_warn() {
        for (name, declarations, written, warns) in [
            (
                "shadowed by another declaration",
                &[("assets/logo.svg", "/assets/brand.svg")][..],
                "/assets/brand.svg",
                true,
            ),
            (
                "published source path",
                &[("assets/logo.svg", "/assets/logo.svg")][..],
                "/assets/logo.svg",
                false,
            ),
            (
                "undeclared path",
                &[("assets/logo.svg", "/assets/logo.svg")][..],
                "/assets/plain.svg",
                false,
            ),
            (
                "directory path",
                &[("assets/logo.svg", "/assets/brand.svg")][..],
                "/assets/brand",
                false,
            ),
        ] {
            let (_directory, _config, diagnostics) = shadowed_path_site(
                declarations
                    .iter()
                    .map(|(source, url)| (std::path::PathBuf::from(source), *url))
                    .collect(),
                written,
            );
            let warnings = shadowed_path_warnings(&diagnostics);
            assert_eq!(warnings.len(), usize::from(warns), "{name}");
            if warns {
                let location = warnings[0].location.as_ref().unwrap();
                assert_eq!(location.path, "site.typ", "{name}");
                assert_eq!(location.line, Some(3), "{name}");
                assert!(
                    diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.severity == Severity::Warning),
                    "{name}: {diagnostics:#?}"
                );
            }
        }
    }

    /// The same warning reaches a published build that reuses its compilation.
    #[test]
    fn published_reuse_keeps_shadow_warning() {
        let (_directory, config, _) = shadowed_path_site(
            vec![("assets/logo.svg".into(), "/assets/brand.svg")],
            "/assets/brand.svg",
        );
        let mut session = crate::BuildSession::new();
        let first = session
            .build_and_write(&config, crate::build::BuildRequest::default())
            .unwrap();
        let second = session
            .build_and_write(
                &config,
                crate::build::BuildRequest {
                    trigger: crate::build::BuildTrigger::Paths(Arc::from([])),
                    reuse: crate::build::BuildReuse {
                        content_inventory: true,
                        typst_compilation: true,
                        configured_assets: true,
                    },
                    ..crate::build::BuildRequest::default()
                },
            )
            .unwrap();
        let first_warnings = shadowed_path_warnings(first.diagnostics());
        assert_eq!(first_warnings.len(), 1);
        assert_eq!(shadowed_path_warnings(second.diagnostics()), first_warnings);
    }

    /// A cancelled revision reports cancellation rather than a compilation failure.
    #[test]
    fn cancelled_revision_reports_cancellation() {
        let (_directory, config) = site();
        let mut session = SourceDiagnosticSession::new(config);
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();
        assert!(matches!(
            session.check(Vec::new(), &canceller.token()),
            Err(crate::cancellation::BuildCancelled)
        ));
    }
    #[test]
    fn public_reports_keep_unsaved_evidence() {
        let (_directory, config) = site();
        let source = config.build.content_dir.join("reports.typ");
        fs::write(&source, "Saved body").unwrap();
        let text = r#"#import "@tola/diagnostic:0.0.0": report
#let value = emph[unsaved 🧭]
#report("unknown variable", severity: "error", code: "check.unused_import/中文", at: value,
  hints: ("A correction",), related: ((message: "Source context", at: value), (message: "No location", at: none)))
#report("later info", severity: "info", code: "info")
#report("later hint", severity: "hint", code: "hint")
"#;
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let cancellation = BuildCancellation::new();
        for _ in 0..2 {
            let revision = session
                .inspect(vec![(source.clone(), Arc::from(text))], &cancellation)
                .unwrap();
            assert!(
                !revision.compiled(),
                "an accumulated error prevents success"
            );
            let reports: Vec<_> = revision
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.producer.is_some())
                .collect();
            assert_eq!(reports.len(), 3, "all reports survive: {reports:#?}");
            let issue = reports
                .iter()
                .find(|diagnostic| diagnostic.severity == Severity::Error)
                .unwrap();
            assert_eq!(
                issue.producer.as_ref().unwrap().name.as_deref(),
                Some("project")
            );
            assert_eq!(
                issue.producer.as_ref().unwrap().code.as_deref(),
                Some("check.unused_import/中文")
            );
            assert!(
                issue.cause.is_none(),
                "public wording cannot create a core correction"
            );
            assert_eq!(issue.help.len(), 1);
            assert_eq!(issue.related.len(), 2);
            assert!(issue.related[1].location.is_none());
            let at = issue.location.as_ref().unwrap();
            assert_eq!(at.path, "content/reports.typ");
            assert!(at.range.is_some());
            assert!(
                at.source_lines
                    .iter()
                    .any(|line| line.text.contains("unsaved 🧭"))
            );
            assert!(
                reports
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Info)
            );
            assert!(
                reports
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Hint)
            );
            assert_eq!(fs::read_to_string(&source).unwrap(), "Saved body");
        }
        assert!(!config.build.publish_dir.exists());
    }

    #[test]
    fn realization_errors_block_publication() {
        let (_directory, config) = site();
        fs::write(
            &config.build.entry,
            r#"#import "@tola/diagnostic:0.0.0": report
#document("index.html")[#context {
  report("stop publication", severity: "error", code: "publish", at: none)
  report("still collected", severity: "hint", code: "context")
}]
"#,
        )
        .unwrap();
        let mut session = crate::BuildSession::new();
        let failed = session
            .build_and_write(&config, crate::build::BuildRequest::default())
            .expect_err("a report error must fail the candidate");
        let diagnostics =
            tola_diagnostic::attached(&failed).expect("structured reports remain attached");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "stop publication"
                    && diagnostic.severity == Severity::Error
                    && diagnostic.location.is_none())
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "still collected"
                    && diagnostic.severity == Severity::Hint)
        );
        assert!(!config.build.publish_dir.exists());
    }

    #[test]
    fn independent_error_not_successful_compilations() {
        let (_directory, config) = documents_workspace();
        let source = config.get_root().join("page.typ");
        let text = "#import \"@tola/diagnostic:0.0.0\": report\n#report(\"source issue\", severity: \"error\")";
        let mut session = SourceDiagnosticSession::for_documents(
            Arc::clone(&config),
            crate::BuildResources::default(),
        );
        let revision = session
            .inspect_source(
                "page.typ",
                vec![(source, Arc::from(text))],
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(!revision.compiled());
        assert!(
            revision
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "source issue"
                    && diagnostic.severity == Severity::Error)
        );
    }

    #[test]
    fn repeated_issues_merge_all_context() {
        let (_directory, config) = site();
        fs::write(config.build.content_dir.join("reports.typ"), r#"#import "@tola/diagnostic:0.0.0": report
#let value = emph[one source]
#for note in ("first", "second") {
  report("same issue", code: "same", at: value, hints: (note,), related: ((message: note, at: value),))
}
"#).unwrap();
        let mut session = SourceDiagnosticSession::new(config);
        let revision = session
            .inspect(Vec::new(), &BuildCancellation::new())
            .unwrap();
        assert!(revision.compiled(), "warnings do not prevent success");
        let reports: Vec<_> = revision
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.producer.is_some())
            .collect();
        assert_eq!(reports.len(), 1, "{reports:#?}");
        assert_eq!(reports[0].help.len(), 2);
        assert_eq!(reports[0].related.len(), 2);
    }
    #[test]
    fn error_wording_cannot_hide_failures() {
        let (_directory, config) = site();
        fs::write(
            config.build.content_dir.join("reports.typ"),
            r#"#import "@tola/diagnostic:0.0.0": report
#report("bundle export is experimental", severity: "error", at: none)
#report("html export is under active development and incomplete", severity: "info", at: none)
"#,
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(config);
        let revision = session
            .inspect(Vec::new(), &BuildCancellation::new())
            .unwrap();
        assert!(!revision.compiled());
        assert!(
            revision
                .diagnostics()
                .iter()
                .any(
                    |diagnostic| diagnostic.message == "bundle export is experimental"
                        && diagnostic.severity == Severity::Error
                )
        );
        assert!(
            revision
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message
                    == "html export is under active development and incomplete"
                    && diagnostic.severity == Severity::Info)
        );
    }
}
