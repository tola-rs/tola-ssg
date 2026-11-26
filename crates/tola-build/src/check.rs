//! Read-only compiler diagnostics for one editor revision, even when the site does not compile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::compiler::SiteHost;
use crate::config::SiteConfig;
use crate::content::ContentUnit;
use tola_diagnostic::Diagnostic;

mod revision;
use revision::{CheckReads, CheckStop, Evaluation, SourceCheck, SourceCheckInput};
pub use revision::{RealizedDocument, SourceCompilation, SourceRevision};

/// Derivation generations the process-wide compiler cache retains across one eviction.
const RETAINED_DERIVATION_GENERATIONS: usize = 1;

/// Compiler resources for read-only checks of successive editor revisions.
pub struct SourceDiagnosticSession {
    config: Arc<SiteConfig>,
    site: Option<Arc<crate::modules::LoadedSite>>,
    resources: crate::resources::BuildResources,
    environment: SourceEnvironment,
    host: Option<SiteHost>,
    icons: Option<crate::icon::IconInventory>,
    analysis: Option<AnalysisReads>,
    reused_analysis: bool,
    /// One-shot queries supply `site` directly; skip re-preparing modules.
    external_site: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceEnvironment {
    Site,
    Documents,
}

struct AnalysisReads {
    cache: crate::compiler::analysis::SourceAnalysisCache,
    deps: crate::compiler::CompilationDependencies,
    unsaved: BTreeMap<PathBuf, Arc<str>>,
}

impl AnalysisReads {
    fn decide(
        &self,
        content: &[ContentUnit],
        bindings: &crate::package::SiteBindings,
        unsaved: &BTreeMap<PathBuf, Arc<str>>,
        boundary: &tola_typst::SourceBoundary,
        cancel: &BuildCancellation,
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

        let mut changed = self.deps.changed_physical_paths(boundary, cancel)?;
        changed.extend(self.deps.changed_package_paths(cancel)?);
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
            &self.deps, &changed,
        )))
    }

    fn of(
        evaluated: &crate::compiler::bundle::EvaluatedSiteProgram,
        compilation: &tola_typst::BundleCompilation,
        cfg: &SiteConfig,
        content: &[ContentUnit],
        unsaved: &BTreeMap<PathBuf, Arc<str>>,
        host: &SiteHost,
        cancel: &BuildCancellation,
    ) -> anyhow::Result<Self> {
        cancel.ensure_active()?;
        let readers = evaluated.dependency_readers(compilation);
        let pkg_checks = evaluated.package_checks(compilation).cloned();
        let deps = crate::compiler::CompilationDependencies::new(
            cfg.root(),
            content.iter().map(|u| u.source.clone()),
            std::iter::once(cfg.build.entry.clone()),
            readers,
            pkg_checks,
            host,
            None,
        )?;
        Ok(Self {
            cache: evaluated.source_analysis().clone(),
            deps,
            unsaved: unsaved.clone(),
        })
    }
}

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
    fn keep_failed_world(
        &mut self,
        world: Option<Arc<tola_typst::TypstWorld>>,
        err: anyhow::Error,
    ) -> anyhow::Error {
        self.world.clone_from(&world);
        err
    }
}

impl SourceDiagnosticSession {
    /// Create a session for one resolved configuration.
    pub fn new(config: Arc<SiteConfig>) -> Self {
        Self::with_resources(config, crate::resources::BuildResources::default())
    }

    pub fn with_resources(
        config: Arc<SiteConfig>,
        resources: crate::resources::BuildResources,
    ) -> Self {
        Self {
            config,
            site: None,
            resources,
            environment: SourceEnvironment::Site,
            host: None,
            icons: None,
            analysis: None,
            reused_analysis: false,
            external_site: false,
        }
    }

    /// A one-shot query already selected its entry from this prepared site.
    /// Its caller owns the manifest freshness checks around inspection.
    pub(crate) fn with_prepared_modules(
        site: Arc<crate::modules::LoadedSite>,
        resources: crate::resources::BuildResources,
    ) -> Self {
        let mut session = Self::with_resources(site.config_arc().clone(), resources);
        session.site = Some(site);
        session.external_site = true;
        session
    }

    /// Create a session for independent documents without a site's content inventory.
    pub fn for_documents(
        config: Arc<SiteConfig>,
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
            host.evict_stale_file_cache_entries(SiteHost::RETAINED_FILE_CACHE_EPOCHS);
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

    fn prepared(
        &mut self,
        cfg: &SiteConfig,
        overrides: &crate::filesystem::SourceOverrides,
        cancel: &BuildCancellation,
    ) -> anyhow::Result<(SiteHost, Vec<ContentUnit>, crate::package::SiteBindings)> {
        let site = self
            .site
            .as_ref()
            .expect("sources are prepared before the compiler runs");
        let boundary = self.resources.source_boundary(cfg);
        if self.environment == SourceEnvironment::Site {
            boundary.check(&cfg.build.content_dir)?;
        }
        for path in overrides
            .paths()
            .chain(cfg.assets.tree_sources())
            .chain(cfg.assets.file_sources())
        {
            boundary.check(path)?;
        }
        let reusable = self
            .host
            .as_ref()
            .filter(|host| host.matches(cfg, &self.resources))
            .map(|host| host.font_inventory_is_fresh(cancel))
            .transpose()?
            .unwrap_or(false);
        if !reusable {
            let host = SiteHost::for_config(
                cfg,
                &self.resources,
                Arc::clone(site.local_packages()),
                cancel,
            )
            .map_err(tola_typst::CompileError::from)?;
            cancel.ensure_active()?;
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
                    cfg, cancel,
                )?;
                crate::content::content_units_with_sources(
                    cfg,
                    &content,
                    overrides.paths().map(PathBuf::from),
                    cancel,
                )?
            }
            SourceEnvironment::Documents => Vec::new(),
        };
        let icons = crate::package::prepare_icons(
            cfg,
            &self.resources,
            cancel,
            &mut self.icons,
            overrides,
        )?;
        let host = host
            .with_icons(icons.collections())
            .with_local_packages(Arc::clone(site.local_packages()));
        let bindings = crate::package::SiteBindings::from_config(
            cfg,
            crate::asset::AssetUrls::for_check_with(cfg, site.applied_assets(), &boundary, cancel)?,
            None,
        )
        .prepare_modules(site.prepared())?;
        Ok((host, content, bindings))
    }

    fn check_configuration(
        &mut self,
        overlays: &[(PathBuf, Arc<str>)],
        cancel: &BuildCancellation,
    ) -> Result<Arc<SiteConfig>, CheckStop> {
        cancel.ensure_active().map_err(CheckStop::Cancelled)?;
        if self.external_site {
            return Ok(Arc::clone(&self.config));
        }
        let bundle = cancel.bundle_cancellation();
        let site = crate::modules::site::prepare_site_with_bundle(
            &self.config,
            self.resources.input_mode(),
            &bundle,
            overlays,
        )
        .map_err(|err| {
            CheckStop::Diagnostics(classify_source_error(&err, self.config.root(), false))
        })?;
        let cfg = Arc::clone(site.config_arc());
        self.site = Some(Arc::new(site));
        self.host = None;
        self.analysis = None;
        self.reused_analysis = false;
        Ok(cfg)
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

    /// Check documents from one editor revision, sharing the host and unsaved sources.
    /// Each result is produced on demand so the caller can drop its compiled world before
    /// compiling the next document.
    pub fn inspect_sources<'a, T>(
        &mut self,
        sources: impl Iterator<Item = (&'a str, T)>,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> impl Iterator<Item = Result<(T, SourceRevision), BuildCancelled>> {
        let mut sources = sources.peekable();
        let prepared = sources
            .peek()
            .map(|_| self.prepare_source_check(overlays, cancellation));
        sources.map(move |(rel, doc)| {
            cancellation.ensure_active()?;
            let rev = match prepared.as_ref().expect("the check has a source") {
                Ok(prepared) => Self::inspect_prepared_source(
                    prepared,
                    SourceCheckInput::Path(rel),
                    cancellation,
                )?,
                Err(CheckStop::Cancelled(_)) => return Err(BuildCancelled),
                Err(CheckStop::Diagnostics(diag)) => SourceRevision::unprepared(diag.clone()),
            };
            Ok((doc, rev))
        })
    }

    fn inspect_source_input(
        &mut self,
        src: SourceCheckInput<'_>,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancel: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        match self.prepare_source_check(overlays, cancel) {
            Ok(prepared) => Self::inspect_prepared_source(&prepared, src, cancel),
            Err(CheckStop::Cancelled(err)) => Err(err),
            Err(CheckStop::Diagnostics(diag)) => Ok(SourceRevision::unprepared(diag)),
        }
    }

    fn prepare_source_check(
        &mut self,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancel: &BuildCancellation,
    ) -> Result<SourceCheck, CheckStop> {
        let cfg = self.check_configuration(&overlays, cancel)?;
        let prepared = crate::filesystem::SourceOverrides::new(overlays, cancel).and_then(|ov| {
            let (host, content, bindings) = self.prepared(&cfg, &ov, cancel)?;
            let sources = crate::content::SourceSet::without_metadata(&content, &cfg)?;
            Ok((host, bindings.library(sources.inputs().to_source_records())))
        });
        cancel.ensure_active().map_err(CheckStop::Cancelled)?;
        match prepared {
            Ok((host, library)) => Ok(SourceCheck { cfg, host, library }),
            Err(err) => Err(CheckStop::Diagnostics(classify_source_error(
                &err,
                cfg.root(),
                false,
            ))),
        }
    }

    fn inspect_prepared_source(
        prepared: &SourceCheck,
        src: SourceCheckInput<'_>,
        cancel: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        let cfg = prepared.cfg.as_ref();
        let host = &prepared.host;
        let mut diag = Vec::new();
        let mut accessed = CheckReads::default();
        let resolved =
            (|| -> anyhow::Result<Option<(Arc<tola_typst::TypstWorld>, typst::syntax::FileId)>> {
                let files = host.file_resolver();
                let main = match src {
                    SourceCheckInput::File(id) => id,
                    SourceCheckInput::Path(rel) => {
                        let path = tola_typst::normalize_path(&cfg.root().join(rel));
                        host.source_boundary().check(&path)?;
                        let main = tola_typst::file_id_from_path(&path, cfg.root())
                            .or_else(|| match files.local_package_ids(&path).ok()?.as_slice() {
                                [id] => Some(*id),
                                _ => None,
                            })
                            .or_else(|| {
                                crate::filesystem::package_document_id(
                                    &path,
                                    Some(cfg.package_locations()),
                                )
                            });
                        let Some(main) = main else { return Ok(None) };
                        main
                    }
                };
                let explicit = if matches!(main.root(), typst::syntax::VirtualRoot::Project) {
                    vec![cfg.root().join(main.vpath().get_without_slash())]
                } else {
                    Vec::new()
                };
                let loaded = tola_typst::ExplicitSources::build_with_files_cancellable(
                    &explicit,
                    cfg.root(),
                    files,
                    &cancel.bundle_cancellation(),
                )
                .map_err(|failure| {
                    let (err, observed) = failure.into_error_and_accessed();
                    accessed.record(&observed);
                    anyhow::Error::new(tola_typst::CompileError::from(err))
                })?;
                let (sources, observed) = loaded.into_sources_and_accessed();
                accessed.record(&observed);
                let candidate_files = host.candidate_files(Arc::new(sources));
                let world = Arc::new(host.world_with_id(
                    cfg.root(),
                    main,
                    &prepared.library,
                    candidate_files,
                    &cancel.bundle_cancellation(),
                )?);
                Ok(Some((world, main)))
            })();
        cancel.ensure_active()?;
        let prepared = match resolved {
            Ok(Some(resolved)) => Some(resolved),
            Ok(None) => None,
            Err(err) => {
                diag.extend(classify_source_error(&err, cfg.root(), false));
                None
            }
        };
        let Some((world, main)) = prepared else {
            return Ok(SourceRevision {
                diagnostics: diag,
                checked: None,
                accessed,
            });
        };
        let mut warnings = tola_typst::Diagnostics::new();
        // A non-compiling source still resolves its imports, so retain `world` for editor queries.
        let evaluation = match tola_typst::compile_source_with_evidence(&world, main) {
            Ok(compiled) => {
                warnings.extend_distinct(compiled.diagnostics());
                let (doc, observed, _) = compiled.into_parts();
                accessed.record(&observed);
                Some(Evaluation::Source {
                    document: Box::new(doc),
                    file_reads: observed.reads,
                })
            }
            Err(failure) => {
                if failure.is_cancelled() {
                    return Err(BuildCancelled);
                }
                let (observed, err) = failure.into_parts();
                accessed.record(&observed);
                diag.extend(classify_source_error(
                    &anyhow::Error::new(err),
                    cfg.root(),
                    false,
                ));
                None
            }
        };
        diag.extend(crate::compiler::warning_diagnostics(cfg, &mut warnings));
        tola_diagnostic::coalesce_reports(&mut diag);
        Ok(SourceRevision {
            diagnostics: diag,
            accessed,
            checked: Some(SourceCompilation {
                world,
                site_entry: None,
                evaluation,
            }),
        })
    }

    fn evaluate_revision(
        &mut self,
        cfg: &SiteConfig,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancel: &BuildCancellation,
        eval: &mut RevisionEvaluation,
    ) -> anyhow::Result<SourceCompilation> {
        let ov = crate::filesystem::SourceOverrides::new(overlays, cancel)?;
        let unsaved: BTreeMap<_, _> = ov
            .sources()
            .map(|(path, src)| (path.to_path_buf(), Arc::clone(&src.text)))
            .collect();
        let (host, content, bindings) = self.prepared(cfg, &ov, cancel)?;
        let mut prepared = None;
        let mut reuse = crate::compiler::analysis::SourceAnalysisReuse::None;
        if let Some(held) = &self.analysis
            && let Some(decision) = held.decide(
                &content,
                &bindings,
                &unsaved,
                host.source_boundary(),
                cancel,
            )?
        {
            eval.reused_site_program = decision.reuses_site_program()
                && held.deps.virtual_reads_match(
                    |ev| match ev.locator() {
                        tola_typst::ReadLocator::ProvidedRoot(rel) => ov
                            .get_physical(&tola_input::normalize_existing_prefix(
                                &cfg.root().join(rel),
                            ))
                            .is_some_and(|src| src.digest == ev.digest()),
                        _ => host.virtual_read_matches(ev),
                    },
                    cancel,
                )?;
            prepared = Some(
                held.cache
                    .clone()
                    .prepare_for_rebuild(&decision, cfg.project_entry_path().as_deref()),
            );
            reuse = crate::compiler::analysis::SourceAnalysisReuse::retained(
                prepared
                    .as_ref()
                    .expect("the reuse holds its prepared cache"),
                &held.deps,
            );
        }
        let evaluated = crate::compiler::bundle::evaluate(
            cfg,
            &host,
            &content,
            // Checks render no configured assets; declarations resolve to mounted names.
            bindings,
            &cancel.bundle_cancellation(),
            reuse,
            &mut eval.reads,
        )
        .map_err(|failure| eval.keep_failed_world(failure.world, failure.error))?;
        self.reused_analysis = prepared.is_some()
            && evaluated
                .source_analysis()
                .reused_dependency_readers()
                .len()
                == content.len();
        eval.warnings
            .extend_distinct(evaluated.source_analysis().diagnostics());
        eval.declaration_warnings
            .extend_from_slice(evaluated.source_analysis().declaration_warnings());
        let realized = crate::compiler::bundle::realize(
            cfg,
            &host,
            &cancel.bundle_cancellation(),
            &evaluated,
            &mut eval.reads,
        )
        .map_err(|failure| eval.keep_failed_world(failure.world, failure.error))?;
        eval.warnings
            .extend_distinct(realized.compilation.diagnostics());
        for (_, reads, _) in evaluated.dependency_readers(&realized.compilation) {
            eval.file_reads.extend(reads);
        }
        self.analysis = Some(AnalysisReads::of(
            &evaluated,
            &realized.compilation,
            cfg,
            &content,
            &unsaved,
            &host,
            cancel,
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
        let cfg = match self.check_configuration(&overlays, cancellation) {
            Ok(cfg) => cfg,
            Err(CheckStop::Cancelled(err)) => return Err(err),
            Err(CheckStop::Diagnostics(diag)) => return Ok(SourceRevision::unprepared(diag)),
        };
        let cfg = cfg.as_ref();
        self.reused_analysis = false;
        let mut eval = RevisionEvaluation::default();
        let checked = self.evaluate_revision(cfg, overlays, cancellation, &mut eval);
        // Repeated queries keep their derivations; an edit ages superseded calculations.
        if !eval.reused_site_program || !self.reused_analysis || checked.is_err() {
            typst::comemo::evict(RETAINED_DERIVATION_GENERATIONS);
        }
        cancellation.ensure_active()?;
        let mut diag = eval.declaration_warnings;
        diag.extend(crate::compiler::warning_diagnostics(
            cfg,
            &mut eval.warnings,
        ));
        tola_diagnostic::coalesce_reports(&mut diag);
        let mut file_reads = eval.file_reads;
        file_reads.extend(eval.reads.successful_reads().cloned());
        let mut seen = std::collections::HashSet::new();
        file_reads.retain(|read| seen.insert(read.clone()));
        let missing_disk_reads = eval.reads.take_missing_disk_reads();
        let (disk_reads, package_checks) = eval.reads.into_parts();
        let accessed = CheckReads {
            disk_reads,
            missing_disk_reads,
            package_checks,
            file_reads,
        };
        match checked {
            Ok(compilation) => Ok(SourceRevision {
                diagnostics: diag,
                checked: Some(compilation),
                accessed,
            }),
            Err(err) if crate::cancellation::is_cancelled(&err) => Err(BuildCancelled),
            Err(err) => {
                diag.extend(classify_source_error(&err, cfg.root(), true));
                tola_diagnostic::coalesce_reports(&mut diag);
                Ok(SourceRevision {
                    diagnostics: diag,
                    accessed,
                    checked: eval.world.map(|world| SourceCompilation {
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

fn classify_source_error(err: &anyhow::Error, root: &Path, include_icons: bool) -> Vec<Diagnostic> {
    tola_diagnostic::attached(err)
        .map(<[Diagnostic]>::to_vec)
        .or_else(|| crate::compiler::error_diagnostics(err, root))
        .or_else(|| {
            if include_icons {
                crate::icon::error_diagnostic(err, root).map(|d| vec![d])
            } else {
                tola_module::error_diagnostic(err).map(|d| vec![d])
            }
        })
        .unwrap_or_else(|| {
            vec![tola_diagnostic::fallback(
                tola_diagnostic::codes::check::SOURCE,
                err,
            )]
        })
}

#[cfg(test)]
mod tests;
