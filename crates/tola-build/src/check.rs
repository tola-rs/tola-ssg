//! Read-only compiler diagnostics for one editor revision.
//!
//! Uses build input preparation, source analysis, and Bundle compilation without
//! running external commands or writing output. A revision that cannot compile the
//! site still resolves the world its own imports, packages, and fonts come from, and
//! one of its sources compiles in that world on its own: an editor answers about the
//! file the author is editing, not only about sites that compile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::compiler::TypstHost;
use crate::config::ResolvedSiteConfig;
use crate::content::ContentUnit;
use crate::diagnostic::Diagnostic;

/// Memoized derivations one eviction keeps in the process-wide compiler cache.
///
/// `comemo` ages every result once per eviction and keeps it while its age stays within the
/// bound, so one generation retains what the revision that evicted touched and releases the
/// revisions it superseded.
const RETAINED_DERIVATION_GENERATIONS: usize = 1;

/// Compiler resources for read-only checks of successive editor revisions.
///
/// One resolved configuration is used throughout the session. Unsaved source
/// text, local icon collections, and ignore rules share one input view.
/// Successfully prepared fonts and icons survive source errors.
pub struct SourceDiagnosticSession {
    config: Arc<ResolvedSiteConfig>,
    resources: crate::resources::BuildResources,
    environment: SourceEnvironment,
    host: Option<TypstHost>,
    icons: Option<crate::icon::IconInventory>,
    analysis: Option<CheckAnalysisEvidence>,
    reused_analysis: bool,
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
}

impl SourceRevision {
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
        self.checked
            .as_ref()
            .is_some_and(|checked| checked.evaluation.is_some())
    }
}

/// A checked site: the world that resolved it, and the Bundle when the root program compiled.
///
/// Source checks do not export outputs, run hooks, or publish revisions. The world outlives a
/// failed compilation, because an editor answers about a file the site's own program never
/// reached, and that file compiles on its own against the same imports, packages, and fonts.
pub struct SourceCompilation {
    world: Arc<tola_typst::TypstWorld>,
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

impl SourceCompilation {
    /// The immutable world containing the inputs this check resolved.
    pub fn world(&self) -> &tola_typst::TypstWorld {
        &self.world
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

    /// The addressable documents this compilation realized, in document order.
    ///
    /// Only documents that hold HTML are listed: a document the site does not serve as a page
    /// has no route. A caller attributes a route to the sources it was realized from by matching
    /// [`RealizedDocument::sources`]; one source can appear in many documents.
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
    /// The file identities the document's original body has, before target realization.
    ///
    /// These describe provenance, not ownership: a document that includes a template has the
    /// template's identity too, and a source included by many documents appears in all of them.
    pub sources: Vec<typst::syntax::FileId>,
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

impl SourceDiagnosticSession {
    /// Create a session for one resolved configuration.
    ///
    /// Create another session after changing configuration. Font changes are
    /// detected by each check and do not require replacing the session.
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
        }
    }

    /// Create a session for independent documents without a site's content inventory.
    ///
    /// The configuration supplies the workspace root, fonts, and package access.
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

    /// Release the parsed sources one session stopped reading.
    ///
    /// The file cache is shared for the life of a session, so a file a reader asked for outside a
    /// revision's own sources would otherwise stay parsed until the process exits. One call ends a
    /// revision; a slot a live world still references is kept, so no answer changes. Site content
    /// sources are not in this cache — a check serves those from its candidate sources — so this
    /// releases imported files, not the explicitly captured content sources and entry program.
    pub fn evict_stale_file_cache_entries(&self) {
        if let Some(host) = &self.host {
            host.evict_stale_file_cache_entries(TypstHost::RETAINED_FILE_CACHE_EPOCHS);
        }
    }

    /// Check one immutable set of unsaved sources without changing source files.
    ///
    /// Overlay paths are absolute filesystem paths; aliases with different text
    /// for the same physical file are rejected. Compiler and input errors
    /// are returned as diagnostics; a cancelled revision returns `BuildCancelled`
    /// so consumers can retain the diagnostics from their last completed check.
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
        let host = host.with_icons(packages.icons.collections());
        let bindings = crate::package::SiteBindings::from_config(
            config,
            crate::asset::AssetUrls::for_check(config, cancellation)?,
        );
        Ok((host, content, bindings))
    }

    /// Check one source on its own, with the diagnostics it produced.
    ///
    /// Site sessions supply their content records even when the root program does not compile.
    /// Document sessions use the workspace's fonts and packages without discovering site content.
    pub fn inspect_source(
        &mut self,
        relative: &str,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        cancellation.ensure_active()?;
        let configuration = Arc::clone(&self.config);
        let config = configuration.as_ref();
        let mut diagnostics = Vec::new();
        let prepared =
            (|| -> anyhow::Result<Option<(Arc<tola_typst::TypstWorld>, typst::syntax::FileId)>> {
                let overrides = crate::filesystem::SourceOverrides::new(overlays, cancellation)?;
                let (host, content, bindings) = self.prepared(config, &overrides, cancellation)?;
                let sources = crate::content::SourceSet::without_metadata(&content, config)?;
                let library = bindings.library(sources.inputs().to_source_records());
                let path = config.get_root().join(relative);
                let files = host.file_resolver();
                let loaded = tola_typst::ExplicitSources::build_with_files_cancellable(
                    std::slice::from_ref(&path),
                    config.get_root(),
                    files,
                    &cancellation.bundle_cancellation(),
                )
                .map_err(|failure| {
                    let (error, _) = failure.into_error_and_accessed();
                    anyhow::Error::new(tola_typst::CompileError::from(error))
                })?;
                let (sources, _) = loaded.into_sources_and_accessed();
                let candidate_files = host.candidate_files(Arc::new(sources));
                let world = Arc::new(host.world(
                    config.get_root(),
                    &path,
                    &library,
                    candidate_files,
                    &cancellation.bundle_cancellation(),
                )?);
                // The id is the one this world resolves the file at, which is how the editor names it.
                let main = tola_typst::file_id_from_path(&path, config.get_root()).or_else(|| {
                    crate::filesystem::package_document_id(&path, Some(config.package_locations()))
                });
                Ok(main.map(|main| (world, main)))
            })();
        cancellation.ensure_active()?;
        let (world, main) = match prepared {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                return Ok(SourceRevision {
                    diagnostics,
                    checked: None,
                });
            }
            Err(error) => {
                diagnostics.extend(source_failure_diagnostics(&error, config.get_root()));
                return Ok(SourceRevision {
                    diagnostics,
                    checked: None,
                });
            }
        };
        let mut warnings = tola_typst::Diagnostics::new();
        // A source that does not compile on its own still resolves what it imports, so the world
        // is what an editor answers from; the evaluation is present only when the file compiled.
        let evaluation = match tola_typst::compile_source_with_evidence(&world, main) {
            Ok(compiled) => {
                warnings.extend_distinct(compiled.diagnostics());
                let (document, accessed, _) = compiled.into_parts();
                Some(Evaluation::Source {
                    document: Box::new(document),
                    file_reads: accessed.reads,
                })
            }
            Err(failure) => {
                if failure.is_cancelled() {
                    return Err(BuildCancelled);
                }
                let (_, error) = failure.into_parts();
                diagnostics.extend(source_failure_diagnostics(
                    &anyhow::Error::new(error),
                    config.get_root(),
                ));
                None
            }
        };
        diagnostics.extend(crate::compiler::warning_diagnostics(config, &mut warnings));
        Ok(SourceRevision {
            diagnostics,
            checked: Some(SourceCompilation { world, evaluation }),
        })
    }

    /// Prepare one immutable revision for diagnostics and semantic queries.
    ///
    /// Failed preparation retains reusable resources but never exposes a
    /// successful compilation. Cancellation is distinct from source errors.
    pub fn inspect(
        &mut self,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<SourceRevision, BuildCancelled> {
        cancellation.ensure_active()?;
        let configuration = Arc::clone(&self.config);
        let config = configuration.as_ref();
        let mut diagnostics = Vec::new();
        let mut warnings = tola_typst::Diagnostics::new();
        // A failed evaluation or realization hands back the world it resolved, so a revision still
        // answers about the files that world resolves.
        let mut world: Option<Arc<tola_typst::TypstWorld>> = None;
        self.reused_analysis = false;
        let mut unchanged_program = false;
        let checked = (|| -> anyhow::Result<SourceCompilation> {
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
                unchanged_program = decision.reuses_site_program()
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
                        .prepare_for_rebuild(&decision, Some(config.build.entry.as_path())),
                );
                reuse = crate::compiler::analysis::SourceAnalysisReuse::retained(
                    prepared
                        .as_ref()
                        .expect("the reuse holds its prepared cache"),
                    &held.dependencies,
                );
            }
            let mut reads = crate::compiler::BuildInputs::default();
            let evaluated = crate::compiler::bundle::evaluate(
                config,
                &host,
                &content,
                // A check renders no configured assets, so every declaration
                // resolves to the mounted address of the name the configuration
                // gives it, with no byte identity: a tree member is enumerated
                // from its source directory without being rendered.
                bindings,
                &cancellation.bundle_cancellation(),
                reuse,
                &mut reads,
            )
            .map_err(|failure| {
                world.clone_from(&failure.world);
                failure.error
            })?;
            self.reused_analysis = prepared.is_some()
                && evaluated
                    .source_analysis()
                    .reused_dependency_readers()
                    .len()
                    == content.len();
            warnings.extend_distinct(evaluated.source_analysis().diagnostics());
            diagnostics.extend_from_slice(evaluated.source_analysis().declaration_warnings());
            let realized = crate::compiler::bundle::realize(
                config,
                &host,
                &cancellation.bundle_cancellation(),
                &evaluated,
                &mut reads,
            )
            .map_err(|failure| {
                world.clone_from(&failure.world);
                failure.error
            })?;
            warnings.extend_distinct(realized.compilation.diagnostics());
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
                world: realized.world,
            })
        })();
        // Repeated queries keep their derivations; an edit ages superseded calculations.
        if !unchanged_program || !self.reused_analysis || checked.is_err() {
            typst::comemo::evict(RETAINED_DERIVATION_GENERATIONS);
        }
        cancellation.ensure_active()?;
        diagnostics.extend(crate::compiler::warning_diagnostics(config, &mut warnings));
        match checked {
            Ok(compilation) => Ok(SourceRevision {
                diagnostics,
                checked: Some(compilation),
            }),
            Err(error) if crate::cancellation::is_cancelled(&error) => Err(BuildCancelled),
            Err(error) => {
                // A source check runs configuration, content, package, and compilation stages, so it
                // classifies exactly those producers rather than the whole publication chain.
                let classified = crate::diagnostic::attached(&error)
                    .map(<[crate::diagnostic::Diagnostic]>::to_vec)
                    .or_else(|| {
                        crate::compiler::error_diagnostics(&error, config.get_root()).or_else(
                            || {
                                crate::icon::error_diagnostic(&error, config.get_root())
                                    .map(|diagnostic| vec![diagnostic])
                            },
                        )
                    })
                    .unwrap_or_else(|| {
                        vec![crate::diagnostic::fallback(
                            crate::codes::check::SOURCE,
                            &error,
                        )]
                    });
                diagnostics.extend(classified);
                Ok(SourceRevision {
                    diagnostics,
                    checked: world.map(|world| SourceCompilation {
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
) -> Vec<crate::diagnostic::Diagnostic> {
    crate::diagnostic::attached(error)
        .map(<[crate::diagnostic::Diagnostic]>::to_vec)
        .or_else(|| crate::compiler::error_diagnostics(error, root))
        .unwrap_or_else(|| {
            vec![crate::diagnostic::fallback(
                crate::codes::check::SOURCE,
                error,
            )]
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;

    fn site() -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(
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
        std::fs::write(
            directory.path().join("page.typ"),
            "#let extra = read(\"/data.txt\")\nBody: #extra\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("data.txt"), "payload\n").unwrap();
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
        std::fs::write(
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
        assert_eq!(errors[0].code, crate::codes::typst::COMPILE);
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
                std::fs::write(&source, disk).unwrap();
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
                std::fs::read_to_string(&source).ok().as_deref(),
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
    fn source_checks_alone_when_the_site_fails() {
        let (_directory, config) = site();
        std::fs::write(
            config.build.content_dir.join("document.typ"),
            "#let brand = 1\n",
        )
        .unwrap();
        std::fs::write(
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
        let data = std::fs::canonicalize(directory.path().join("data.txt")).unwrap();
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
        std::fs::write(
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
        std::fs::write(&broken, "#include \"absent.typ\"\n").unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let failed = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            failed
                .checked()
                .is_some_and(|checked| checked.bundle().is_none()),
            "a missing include fails the Bundle"
        );

        std::fs::remove_file(&broken).unwrap();
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
            std::fs::write(&content, "Body.\n").unwrap();
            std::fs::write(&dependency, "Dependency.\n").unwrap();
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
                Change::DiskDependency => {
                    std::fs::write(&dependency, "Changed on disk.\n").unwrap()
                }
                Change::AddedSource => {
                    std::fs::write(directory.path().join("content/added.typ"), "Added.\n").unwrap()
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
        std::fs::write(config.build.content_dir.join("post.typ"), "Body.\n").unwrap();
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
        std::fs::create_dir_all(assets.join("brand")).unwrap();
        for (name, id) in [
            ("logo.svg", "logo"),
            ("plain.svg", "plain"),
            ("brand.svg", "sitting"),
            ("brand/logo.svg", "brand"),
        ] {
            std::fs::write(
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
        std::fs::write(
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
            .filter(|diagnostic| diagnostic.code == crate::codes::reference::ASSET_URL_SHADOWED)
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
}
