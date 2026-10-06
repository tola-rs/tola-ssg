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
    host: Option<TypstHost>,
    icons: Option<crate::icon::IconSnapshot>,
    analysis: Option<CheckAnalysisEvidence>,
    reused_analysis: bool,
}

/// What one successful check derived and what it read.
///
/// A check re-runs on every editor revision. This keeps the revision's source analysis and the
/// dependency evidence that licenses reusing it: which reader read which path, with the digest it
/// observed, so the next revision can reuse the analysis exactly while every path still holds what
/// it read.
struct CheckAnalysisEvidence {
    cache: crate::compiler::analysis::SourceAnalysisCache,
    dependencies: crate::compiler::PublishedDependencies,
    /// The unsaved sources the analysis was derived from, by path.
    unsaved: BTreeMap<PathBuf, Arc<str>>,
    /// Every path the analysis read, with the fingerprint it read.
    reads: BTreeMap<PathBuf, crate::filesystem::PathFingerprint>,
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
    /// The decision that licenses reusing this analysis for the current revision.
    ///
    /// Identity decides first: changed content membership, changed package bindings, or different
    /// unsaved text refuses reuse without reading the filesystem. What remains can only have
    /// changed where the analysis read bytes from disk, so every such read is re-fingerprinted.
    fn decide(
        &self,
        content: &[ContentUnit],
        bindings: &crate::package::SiteBindings,
        unsaved: &BTreeMap<PathBuf, Arc<str>>,
        cancellation: &BuildCancellation,
    ) -> Option<crate::compiler::RebuildDecision> {
        let reason = if !self.cache.matches_content_identity(content) {
            Some("the site's content membership changed")
        } else if !self.cache.matches_package_bindings(bindings) {
            Some("the site's packages or asset addresses changed")
        } else if self.unsaved != *unsaved {
            Some("an unsaved source changed")
        } else {
            None
        };
        if let Some(reason) = reason {
            tracing::debug!(reason, "check re-derives its source analysis");
            return None;
        }

        // The checks above establish that the open documents still hold the text the analysis
        // read, so a source can have changed only where the bytes it read on disk did.
        let mut changed = Vec::new();
        for path in self.dependencies.physical_read_paths() {
            // A path that cannot be fingerprinted is a path that is not what it was.
            let unchanged = crate::filesystem::path_fingerprint(&path, Some(cancellation))
                .is_ok_and(|fingerprint| self.reads.get(&path) == Some(&fingerprint));
            if !unchanged {
                changed.push(path);
            }
        }
        if changed.is_empty() {
            // Nothing the analysis read changed, so nothing needs re-reading: a reader a provider
            // would mark as requiring a rebuild can only be one of this check's own overlays, and
            // every entry is compared against the candidate files before it is reused.
            return Some(crate::compiler::RebuildDecision::for_paths(
                None,
                &[],
                false,
            ));
        }
        tracing::debug!(
            changed = changed.len(),
            "check re-derives its source analysis"
        );
        Some(crate::compiler::RebuildDecision::for_paths(
            Some(&self.dependencies),
            &changed,
            false,
        ))
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
        let dependencies = crate::compiler::PublishedDependencies::new(
            config.get_root(),
            content.iter().map(|unit| unit.source.clone()),
            std::iter::once(config.build.entry.clone()),
            reader_evidence,
            package_checks,
            host,
            None,
        )?;
        let mut reads = BTreeMap::new();
        for path in dependencies.physical_read_paths() {
            reads.insert(
                path.clone(),
                crate::filesystem::path_fingerprint(&path, Some(cancellation))?,
            );
        }
        Ok(Self {
            cache: evaluated.source_analysis().clone(),
            dependencies,
            unsaved: unsaved.clone(),
            reads,
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
            host: None,
            icons: None,
            analysis: None,
            reused_analysis: false,
        }
    }

    pub fn resources(&self) -> &crate::BuildResources {
        &self.resources
    }

    /// Whether the last revision answered from the previous revision's source analysis.
    pub fn reused_source_analysis(&self) -> bool {
        self.reused_analysis
    }

    /// Release the parsed sources one session stopped reading.
    ///
    /// The file cache is shared for the life of a session, so a file a reader asked for outside a
    /// revision's own snapshot would otherwise stay parsed until the process exits. One call ends a
    /// revision; a slot a live world still references is kept, so no answer changes. Site content
    /// sources are not in this cache — a check serves those from its candidate snapshot — so this
    /// releases the entry program and package files, never the site's own documents.
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
        boundary.check(&config.build.content_dir)?;
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
        }
        let host = self
            .host
            .as_ref()
            .expect("compiler resources are ready")
            .with_source_overrides(overrides)?;
        let content = crate::content::discover_content_units_for_config_with_cancellation(
            config,
            cancellation,
        )?;
        let content = crate::content::content_units_with_sources(
            config,
            &content,
            overrides.paths().map(PathBuf::from),
            cancellation,
        )?;
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

    /// Check one source of the site on its own, with the diagnostics it produced.
    ///
    /// A site that does not compile still resolves its own imports, packages, and fonts, so this
    /// check prepares the world the site's own program could not reach and compiles one source in
    /// it. An editor answers about the file the author is editing from exactly that world: what the
    /// file alone establishes, with the diagnostics that name what stopped the file.
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
                let loaded = tola_typst::SourceSnapshot::build_with_files_cancellable(
                    std::slice::from_ref(&path),
                    config.get_root(),
                    files,
                    &cancellation.bundle_cancellation(),
                )
                .map_err(|failure| {
                    let (error, _) = failure.into_error_and_accessed();
                    anyhow::Error::new(tola_typst::CompileError::from(error))
                })?;
                let (snapshot, _) = loaded.into_snapshot_and_accessed();
                let candidate_files = host.candidate_files(Arc::new(snapshot));
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
        let unsaved = overlays
            .iter()
            .map(|(path, text)| (path.clone(), Arc::clone(text)))
            .collect::<BTreeMap<_, _>>();
        let checked = (|| -> anyhow::Result<SourceCompilation> {
            let overrides = crate::filesystem::SourceOverrides::new(overlays, cancellation)?;
            let (host, content, bindings) = self.prepared(config, &overrides, cancellation)?;
            let mut prepared = None;
            let mut reuse = crate::compiler::analysis::SourceAnalysisReuse::None;
            if let Some(held) = &self.analysis
                && let Some(decision) = held.decide(&content, &bindings, &unsaved, cancellation)
                && decision.reuses_source_analysis()
            {
                prepared = Some(
                    held.cache
                        .clone()
                        .prepare_for_rebuild(&decision, Some(config.build.entry.as_path())),
                );
                reuse = crate::compiler::analysis::SourceAnalysisReuse::published(
                    prepared
                        .as_ref()
                        .expect("the reuse holds its prepared cache"),
                    &held.dependencies,
                );
            }
            self.reused_analysis = prepared.is_some();
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
        // The revision has derived everything it can reuse. Whatever it did not touch belongs to
        // the revisions it superseded, so releasing those generations here keeps a session's
        // retained state proportional to the site rather than to the edits made in it. A revision
        // that resumed the previous generation superseded nothing, so it ages nothing: evicting
        // here would release exactly the derivations the next deriving revision resumes from.
        if !self.reused_analysis || checked.is_err() {
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::diagnostic::Severity;

    fn site() -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(
            directory.path().join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();
        let config = Arc::new(crate::config::tests::load_test_config(directory.path(), ""));
        (directory, config)
    }

    fn documents_workspace() -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(
            directory.path().join("content/page.typ"),
            "#let extra = read(\"/data.txt\")\nBody: #extra\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("data.txt"), "payload\n").unwrap();
        let config = Arc::new(crate::config::tests::load_test_config(directory.path(), ""));
        (directory, config)
    }

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

    #[test]
    fn realized_documents_report_identity() {
        let (directory, config) = site();
        let content = directory.path().join("content/post.typ");
        std::fs::write(&content, "Body.\n").unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(config.clone());
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            revision
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{:#?}",
            revision.diagnostics()
        );
        let compilation = revision.checked().expect("a checked compilation");

        let documents = compilation.realized_documents(&cancellation).unwrap();
        assert_eq!(
            documents
                .iter()
                .map(|document| (
                    document.output.as_str().to_owned(),
                    document.permalink.as_str().to_owned()
                ))
                .collect::<Vec<_>>(),
            [("post/index.html".to_owned(), "/post/".to_owned())]
        );
        let source = tola_typst::file_id_from_path(
            &crate::filesystem::normalize_existing_prefix(&content),
            config.get_root(),
        )
        .expect("the content source has a file identity");
        assert!(documents[0].sources.contains(&source), "{:?}", documents[0]);
    }

    #[test]
    fn unsaved_json_icons_replace_collections() {
        let (_directory, mut config) = site();
        let path = config.get_root().join("icons.json");
        Arc::get_mut(&mut config).unwrap().icons.collections.insert(
            "brand".into(),
            crate::config::section::IconCollectionSource::LocalJson { path: path.clone() },
        );
        let disk = r#"{"prefix":"brand","icons":{"old":{"body":"<path d='M0 0h16v16H0z'/>"}}}"#;
        let unsaved = disk.replace("old", "new");
        std::fs::write(&path, disk).unwrap();
        let entry = config.build.entry.clone();
        let root = r#"#import "@tola/icon:0.0.0": icon-bytes
#assert("old" in json("icons.json").icons)
#asset("icon.svg", icon-bytes("brand:old"))
#document("index.html")[Home]"#;
        std::fs::write(&entry, root).unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(config);
        let saved = session.check(Vec::new(), &cancellation).unwrap();
        assert!(
            saved
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{saved:#?}"
        );
        let root = Arc::<str>::from(root.replace("old", "new"));
        let check = |session: &mut SourceDiagnosticSession, json: &str| {
            session
                .check(
                    vec![
                        (path.clone(), Arc::from(json)),
                        (entry.clone(), Arc::clone(&root)),
                    ],
                    &cancellation,
                )
                .unwrap()
        };
        let added = check(&mut session, &unsaved);
        assert!(
            added
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{added:#?}"
        );
        assert!(
            session
                .icons
                .as_ref()
                .unwrap()
                .collections()
                .get("brand", "old")
                .is_none()
        );

        let recolored = unsaved.replace("<path ", "<path fill='red' ");
        let changed = check(&mut session, &recolored);
        assert!(
            changed
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{changed:#?}"
        );
        assert_eq!(
            session
                .icons
                .as_ref()
                .unwrap()
                .collections()
                .get("brand", "new")
                .unwrap()
                .paint(),
            tola_icons::IconPaint::Fixed
        );
        assert!(
            !session
                .icons
                .as_ref()
                .unwrap()
                .evidence()
                .is_fresh(&cancellation)
                .unwrap()
        );
        let completed = session.icons.as_ref().unwrap().collections();

        let invalid = check(&mut session, &recolored.replace("<path ", "<script "));
        assert!(
            invalid
                .iter()
                .any(|diagnostic| diagnostic.code == "icons.collection"
                    && diagnostic.severity == Severity::Error),
            "{invalid:#?}"
        );
        assert!(Arc::ptr_eq(
            &completed,
            &session.icons.as_ref().unwrap().collections()
        ));

        let removed = check(&mut session, disk);
        assert!(
            removed
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error),
            "{removed:#?}"
        );
        assert!(
            session
                .icons
                .as_ref()
                .unwrap()
                .collections()
                .get("brand", "new")
                .is_none()
        );
        let recovered = check(&mut session, &unsaved);
        assert!(
            recovered
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{recovered:#?}"
        );
        let closed = session.check(Vec::new(), &cancellation).unwrap();
        assert!(
            closed
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{closed:#?}"
        );
        assert!(
            session
                .icons
                .as_ref()
                .unwrap()
                .collections()
                .get("brand", "new")
                .is_none()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), disk);
    }

    #[test]
    fn unsaved_svg_members_close_without_writes() {
        let (_directory, mut config) = site();
        let directory = config.get_root().join("brand");
        std::fs::create_dir(&directory).unwrap();
        let saved_path = directory.join("mark.svg");
        let svg = r#"<svg viewBox="0 0 24 24"><path fill="currentColor" d="M0 0h24v24H0z"/></svg>"#;
        std::fs::write(&saved_path, svg).unwrap();
        Arc::get_mut(&mut config).unwrap().icons.collections.insert(
            "brand".into(),
            crate::config::section::IconCollectionSource::LocalSvgDir {
                path: directory.clone(),
            },
        );
        let path = directory.join("nested/new.svg");
        let root = Arc::<str>::from(
            r#"#import "@tola/icon:0.0.0": icon-bytes
#assert(read("brand/nested/new.svg").contains("red"))
#asset("new.svg", icon-bytes("brand:nested-new"))
#asset("mark.svg", icon-bytes("brand:mark"))
#document("index.html")[Home]"#,
        );
        let entry = config.build.entry.clone();
        let mut session = SourceDiagnosticSession::new(config);
        let cancellation = BuildCancellation::new();
        let saved = session.check(Vec::new(), &cancellation).unwrap();
        assert!(
            saved
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{saved:#?}"
        );
        let red = Arc::<str>::from(svg.replace("currentColor", "red"));
        let overlays = vec![
            (path.clone(), Arc::clone(&red)),
            (saved_path.clone(), Arc::clone(&red)),
            (entry.clone(), root),
        ];
        let added = session.check(overlays.clone(), &cancellation).unwrap();
        assert!(
            added
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{added:#?}"
        );
        let collections = session.icons.as_ref().unwrap().collections();
        assert_eq!(
            collections.get("brand", "mark").unwrap().paint(),
            tola_icons::IconPaint::Fixed
        );
        assert!(collections.get("brand", "nested-new").is_some());
        let mut invalid = overlays.clone();
        invalid[0].1 = Arc::from("<svg");
        let invalid = session.check(invalid, &cancellation).unwrap();
        assert!(
            invalid
                .iter()
                .any(|diagnostic| diagnostic.code == "icons.collection"
                    && diagnostic.severity == Severity::Error),
            "{invalid:#?}"
        );
        assert!(Arc::ptr_eq(
            &collections,
            &session.icons.as_ref().unwrap().collections()
        ));
        let recovered = session.check(overlays, &cancellation).unwrap();
        assert!(
            recovered
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{recovered:#?}"
        );
        let closed = session.check(Vec::new(), &cancellation).unwrap();
        assert!(
            closed
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{closed:#?}"
        );
        let collections = session.icons.as_ref().unwrap().collections();
        assert!(collections.get("brand", "nested-new").is_none());
        assert_eq!(
            collections.get("brand", "mark").unwrap().paint(),
            tola_icons::IconPaint::CurrentColor
        );
        assert_eq!(std::fs::read_to_string(saved_path).unwrap(), svg);
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn unsaved_overlays_leave_disk_unchanged() {
        for (name, existing_disk, source, overlay, expect_error) in [
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

            match expect_error {
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

    #[test]
    fn unsaved_text_reuses_resources() {
        let (_directory, config) = site();
        let source = config.build.content_dir.join("document.typ");
        let disk = "Saved";
        std::fs::write(&source, disk).unwrap();
        let mut session = SourceDiagnosticSession::new(config);
        let cancellation = BuildCancellation::new();
        session.check(Vec::new(), &cancellation).unwrap();

        let text = Arc::<str>::from("Unsaved");
        let clean = session
            .check(vec![(source.clone(), Arc::clone(&text))], &cancellation)
            .unwrap();
        assert!(
            clean
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{clean:#?}"
        );
        let completed_icons = session.icons.as_ref().unwrap().collections();
        session
            .check(vec![(source.clone(), text)], &cancellation)
            .unwrap();
        assert!(Arc::ptr_eq(
            &session.icons.as_ref().unwrap().collections(),
            &completed_icons
        ));

        let failed = session
            .check(
                vec![(source.clone(), Arc::from("Changed\n#unsaved_error"))],
                &cancellation,
            )
            .unwrap();
        assert!(
            failed
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error
                    && diagnostic.message.contains("unsaved_error")),
            "{failed:#?}"
        );
        assert!(Arc::ptr_eq(
            &session.icons.as_ref().unwrap().collections(),
            &completed_icons
        ));

        assert_eq!(std::fs::read_to_string(source).unwrap(), disk);
    }

    #[test]
    fn panic_message_keeps_typst_code() {
        let (_directory, config) = site();
        std::fs::write(
            config.build.content_dir.join("post.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: panic(\"is also a published asset URL\")))\n",
        ).unwrap();
        let mut session = SourceDiagnosticSession::new(config);
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
    }

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
    fn source_reuse_keeps_shadow_warning() {
        let (_directory, config, _) = shadowed_path_site(
            vec![("assets/logo.svg".into(), "/assets/brand.svg")],
            "/assets/brand.svg",
        );
        std::fs::write(
            config.build.content_dir.join("post.typ"),
            "#import \"@tola/image:0.0.0\": image-metadata\n\
             #import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"Post\", image: image-metadata(\"/assets/brand.svg\")))\n",
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(config);
        let cancellation = BuildCancellation::default();
        let first = session.check(Vec::new(), &cancellation).unwrap();
        let source_warnings = shadowed_path_warnings(&first)
            .into_iter()
            .filter(|diagnostic| {
                diagnostic
                    .location
                    .as_ref()
                    .is_some_and(|location| location.path == "content/post.typ")
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(source_warnings.len(), 1);
        let second = session.check(Vec::new(), &cancellation).unwrap();
        assert!(session.reused_source_analysis());
        let reused = shadowed_path_warnings(&second)
            .into_iter()
            .filter(|diagnostic| {
                diagnostic
                    .location
                    .as_ref()
                    .is_some_and(|location| location.path == "content/post.typ")
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(reused, source_warnings);
    }

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

    /// An asset URL that another declaration shadows warns; an unambiguous one stays silent.
    #[test]
    fn ambiguous_asset_urls_warn() {
        enum Expected {
            /// One warning at the site program's declaration line, and nothing else.
            Warn,
            /// No shadow warning: the path resolves to its own file.
            Silent,
            /// No shadow warning, but the missing file still reports its own error.
            SilentWithError,
        }
        let cases = [
            (
                "shadowed by another declaration",
                &[("assets/logo.svg", "/assets/brand.svg")][..],
                "/assets/brand.svg",
                Expected::Warn,
            ),
            (
                "published source path",
                &[("assets/logo.svg", "/assets/logo.svg")][..],
                "/assets/logo.svg",
                Expected::Silent,
            ),
            (
                "undeclared path",
                &[("assets/logo.svg", "/assets/logo.svg")][..],
                "/assets/plain.svg",
                Expected::Silent,
            ),
            (
                "missing image",
                &[("assets/logo.svg", "/assets/brand.svg")][..],
                "/assets/absent.svg",
                Expected::SilentWithError,
            ),
            (
                "directory path",
                &[("assets/logo.svg", "/assets/brand.svg")][..],
                "/assets/brand",
                Expected::Silent,
            ),
        ];
        for (name, declarations, written, expected) in cases {
            let (_directory, _config, diagnostics) = shadowed_path_site(
                declarations
                    .iter()
                    .map(|(source, url)| (std::path::PathBuf::from(source), *url))
                    .collect(),
                written,
            );
            let warnings = shadowed_path_warnings(&diagnostics);
            match expected {
                Expected::Warn => {
                    assert_eq!(warnings.len(), 1, "{name}");
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
                Expected::Silent => {
                    assert!(warnings.is_empty(), "{name}: {diagnostics:#?}");
                }
                Expected::SilentWithError => {
                    assert!(warnings.is_empty(), "{name}: {diagnostics:#?}");
                    assert!(
                        diagnostics
                            .iter()
                            .any(|diagnostic| diagnostic.severity == Severity::Error),
                        "{name}: the official file-not-found reports the unresolvable path: {diagnostics:#?}"
                    );
                }
            }
        }
    }

    #[test]
    fn tree_asset_path_is_silent() {
        let (directory, mut config) = site();
        let root = config.get_root().to_path_buf();
        std::fs::create_dir_all(root.join("assets/brand")).unwrap();
        std::fs::write(
            root.join("assets/logo.svg"),
            "<svg width=\"4\" height=\"4\" id=\"logo\"/>",
        )
        .unwrap();
        std::fs::write(
            root.join("assets/brand/logo.svg"),
            "<svg width=\"4\" height=\"4\" id=\"brand\"/>",
        )
        .unwrap();
        let configured = Arc::get_mut(&mut config).unwrap();
        configured.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
            root.join("assets/brand"),
            crate::config::section::AssetUrlPrefix::parse("/assets").unwrap(),
        )];
        let entry = configured.build.entry.clone();
        std::fs::write(
            &entry,
            "#import \"@tola/image:0.0.0\": image-metadata\n#document(\"index.html\")[\n  #metadata(image-metadata(\"/assets/logo.svg\"))\n]\n",
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let revision = session
            .inspect(Vec::new(), &BuildCancellation::new())
            .unwrap();
        let _ = directory;
        assert!(
            shadowed_path_warnings(revision.diagnostics()).is_empty(),
            "a tree member publishes its own file, so there is nothing to compare: {:#?}",
            revision.diagnostics()
        );
    }

    #[test]
    fn checks_resolve_configured_asset_urls() {
        use crate::config::section::{
            AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
        };
        let (_directory, mut config) = site();
        let root = config.get_root().to_path_buf();
        let assets = root.join("assets");
        std::fs::create_dir_all(assets.join("images")).unwrap();
        std::fs::write(assets.join("images/logo.svg"), "<svg></svg>").unwrap();
        std::fs::write(assets.join("app.js"), "const answer = 40 + 2;").unwrap();
        let configured = Arc::get_mut(&mut config).unwrap();
        configured.assets.trees = vec![AssetTreeDeclaration::new(
            &assets,
            AssetUrlPrefix::parse("/assets").unwrap(),
        )];
        configured.assets.files = vec![AssetFileDeclaration::new(
            assets.join("app.js"),
            AssetUrl::parse("/assets/app.js").unwrap(),
        )];
        configured.assets.cache_busting = true;
        configured.build.minify.javascript = true;
        let entry = configured.build.entry.clone();
        std::fs::write(
            &entry,
            r#"#import "@tola/address:0.0.0": asset-url
#document("index.html")[
  #metadata(asset-url("/assets/images/logo.svg")) <tree-member>
  #metadata(asset-url("/assets/app.js")) <declared-file>
]"#,
        )
        .unwrap();

        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let cancellation = BuildCancellation::new();
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            revision
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{:#?}",
            revision.diagnostics()
        );
        let compilation = revision.checked().expect("the check compiled the site");
        let path = typst::syntax::VirtualPath::new("index.html").unwrap();
        let document = compilation
            .bundle()
            .expect("a compiled Bundle")
            .document(&path)
            .expect("the checked site declares its document");
        let resolved = |label: &str| {
            document
                .metadata_unique(label)
                .expect("one resolved URL")
                .expect("the document declares the URL")
                .cast::<typst::foundations::Str>()
                .expect("asset-url returns a string")
                .to_string()
        };
        assert_eq!(resolved("tree-member"), "/assets/images/logo.svg");
        assert_eq!(resolved("declared-file"), "/assets/app.js");

        let unsaved = Arc::<str>::from(
            std::fs::read_to_string(&entry)
                .unwrap()
                .replace("/assets/app.js", "/assets/missing.js"),
        );
        let revision = session
            .inspect(vec![(entry, unsaved)], &cancellation)
            .unwrap();
        // The check keeps the world it resolved, without a compiled Bundle to answer from.
        assert!(
            revision
                .checked()
                .is_some_and(|checked| checked.introspector().is_none())
        );
        assert!(
            revision.diagnostics().iter().any(|diagnostic| {
                diagnostic.severity == Severity::Error
                    && diagnostic.message.contains("/assets/missing.js")
            }),
            "{:#?}",
            revision.diagnostics()
        );
    }

    #[test]
    fn checks_report_only_source_errors() {
        use crate::config::section::{AssetFileDeclaration, AssetUrl};
        let (_directory, mut config) = site();
        let script = config.get_root().join("broken.js");
        std::fs::write(&script, "function {").unwrap();
        let configured = Arc::get_mut(&mut config).unwrap();
        configured.assets.files.push(AssetFileDeclaration::new(
            &script,
            AssetUrl::parse("/app.js").unwrap(),
        ));
        configured.build.minify.javascript = true;
        std::fs::write(
            config.build.content_dir.join("document.typ"),
            "#source_error",
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let diagnostics = session
            .check(Vec::new(), &BuildCancellation::new())
            .unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("source_error")),
            "{diagnostics:?}"
        );
        std::fs::write(config.build.content_dir.join("document.typ"), "Document").unwrap();
        let mut root = std::fs::read_to_string(&config.build.entry).unwrap();
        root.push_str("\n#metadata(42) <tola-feed>\n");
        std::fs::write(&config.build.entry, root).unwrap();
        let diagnostics = session
            .check(Vec::new(), &BuildCancellation::new())
            .unwrap();
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{diagnostics:?}"
        );
        assert!(!config.build.publish_dir.exists());
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

    #[test]
    fn failed_source_reports_its_diagnostics() {
        let (_directory, config) = site();
        std::fs::write(
            config.build.content_dir.join("broken.typ"),
            "#let nothing = absent\n",
        )
        .unwrap();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let cancellation = BuildCancellation::new();

        let revision = session
            .inspect_source("content/broken.typ", Vec::new(), &cancellation)
            .unwrap();

        assert!(
            !revision.diagnostics().is_empty(),
            "the file's failure is reported: {:?}",
            revision.diagnostics()
        );
        assert!(
            revision
                .checked()
                .is_some_and(|checked| checked.introspector().is_none()),
            "the file did not compile"
        );
    }

    /// A workspace that compiles no site still records what the document it checked read from
    /// disk, which is the evidence an editor filters file-change notifications by.
    #[test]
    fn compiled_source_reports_disk_reads() {
        let (directory, config) = documents_workspace();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(config);
        let revision = session
            .inspect_source("content/page.typ", Vec::new(), &cancellation)
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

    #[test]
    fn failed_source_reports_no_reads() {
        let (directory, config) = documents_workspace();
        std::fs::write(
            directory.path().join("content/page.typ"),
            "#let extra = read(\"/data.txt\")\n#absent_name\n",
        )
        .unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(config);
        let revision = session
            .inspect_source("content/page.typ", Vec::new(), &cancellation)
            .unwrap();
        assert!(!revision.compiled(), "{:#?}", revision.diagnostics());
        let reads = revision
            .checked()
            .expect("the document resolved a world")
            .file_reads();
        assert!(
            reads.is_empty(),
            "a failed compilation establishes no read evidence: {reads:?}"
        );
    }

    #[test]
    fn invalid_source_keeps_prepared_resources() {
        let (_directory, config) = site();
        let source = config.build.content_dir.join("document.typ");
        std::fs::write(&source, "#source_error").unwrap();
        let mut session = SourceDiagnosticSession::new(config);
        let cancellation = BuildCancellation::new();
        let first = session.check(Vec::new(), &cancellation).unwrap();
        assert!(
            first
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        let host = session.host.as_ref().unwrap().clone();
        let icons = session.icons.as_ref().unwrap().collections();
        let second = session.check(Vec::new(), &cancellation).unwrap();
        assert!(
            second
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        assert!(host.has_same_inputs(session.host.as_ref().unwrap()));
        assert!(Arc::ptr_eq(
            &icons,
            &session.icons.as_ref().unwrap().collections()
        ));
    }

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
                .any(|(path, _)| { path.get_with_slash() == "/document/index.html" })
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
        assert_eq!(
            compilation.world().source(id).unwrap().text(),
            "Unsaved body"
        );
        assert!(!source.exists());
        assert!(!config.build.publish_dir.exists());
    }

    /// A failed revision still reports the retired label spelling that declared nothing.
    #[test]
    fn failed_revision_reports_the_label_declaration() {
        let (_directory, config) = site();
        std::fs::write(
            config.build.content_dir.join("index.typ"),
            "#metadata((title: \"Home\")) <tola-meta>\n= Home",
        )
        .unwrap();
        std::fs::write(
            &config.build.entry,
            r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let pages = parse-sources(all-sources(), schema((title: str)))
#for page in pages {
  document(page.file, title: page.meta.title)[#include page.file]
}
"#,
        )
        .unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let failed = session.inspect(Vec::new(), &cancellation).unwrap();

        let diagnostics = failed.diagnostics();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("`$.title`: is required")),
            "the site program fails: {diagnostics:#?}"
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == crate::codes::source::DECLARATION_DEPRECATED)
                .count(),
            1,
            "{diagnostics:#?}"
        );
    }

    /// One working revision reports the retired label spelling exactly once.
    #[test]
    fn working_revision_reports_one_label_declaration() {
        let (_directory, config) = site();
        std::fs::write(
            config.build.content_dir.join("post.typ"),
            "#metadata((title: \"Post\")) <tola-meta>",
        )
        .unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();

        assert!(
            revision.checked().is_some(),
            "{:#?}",
            revision.diagnostics()
        );
        assert_eq!(
            revision
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == crate::codes::source::DECLARATION_DEPRECATED)
                .count(),
            1,
            "{:#?}",
            revision.diagnostics()
        );
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
            "a missing include fails the Bundle, not the world its sources resolve"
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

    /// Every computation of [`counted_derivation`], so a released entry is visible.
    static DERIVATION_COMPUTATIONS: AtomicUsize = AtomicUsize::new(0);

    /// A memoized value the test watches: its body runs once per retained cache entry.
    #[comemo::memoize]
    fn counted_derivation(value: u64) -> u64 {
        DERIVATION_COMPUTATIONS.fetch_add(1, Ordering::SeqCst);
        value
    }

    /// Two deriving revisions release a memoized derivation neither of them touched.
    #[test]
    fn deriving_revisions_release_untouched_derivations() {
        let (_directory, config) = site();
        let document = config.build.content_dir.join("document.typ");
        std::fs::write(&document, "Saved.\n").unwrap();
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(config);

        counted_derivation(7);
        counted_derivation(7);
        assert_eq!(
            DERIVATION_COMPUTATIONS.load(Ordering::SeqCst),
            1,
            "the watched value is memoized before any revision"
        );

        // Different unsaved text refuses the previous analysis, so both revisions derive.
        for round in 0..2 {
            let unsaved = Arc::<str>::from(format!("Round {round}.\n"));
            session
                .inspect(vec![(document.clone(), unsaved)], &cancellation)
                .unwrap();
        }
        counted_derivation(7);
        assert_eq!(
            DERIVATION_COMPUTATIONS.load(Ordering::SeqCst),
            2,
            "deriving revisions released no earlier derivation"
        );
    }

    /// A check keeps the site's own documents out of the shared file cache.
    ///
    /// Content sources are served from the check's own candidate snapshot, so the slots the shared
    /// cache owns are the entry program and package files whatever the site's size. Routing content
    /// through it would put every parsed document under the eviction window documented at
    /// [`TypstHost::RETAINED_FILE_CACHE_EPOCHS`], so this pins which files the cache serves.
    #[test]
    fn check_keeps_site_documents_out_of_shared_file_cache() {
        let echo_documents = shared_slots_after_check(4);
        assert!(
            echo_documents > 0,
            "a check that rendered four sources retained no parsed source at all"
        );
        assert_eq!(
            echo_documents,
            shared_slots_after_check(20),
            "a larger content root changed what the shared file cache retains"
        );
    }

    /// The slots the shared file cache owns after one check of a site with `documents` sources.
    fn shared_slots_after_check(documents: usize) -> usize {
        let (_directory, config) = site();
        for index in 0..documents {
            std::fs::write(
                config.build.content_dir.join(format!("doc-{index}.typ")),
                format!("= Document {index}\n"),
            )
            .unwrap();
        }
        let resources = crate::BuildResources::new();
        let cache = resources.file_cache();
        let mut session = SourceDiagnosticSession::with_resources(config, resources);
        session
            .inspect(Vec::new(), &BuildCancellation::new())
            .unwrap();
        cache.retained_slots()
    }

    /// Reuse follows every input the source analysis read: an unsaved revision that repeats its
    /// inputs reuses, and a change to an unsaved source, a read file on disk, or the set of
    /// sources re-derives.
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
                // The unsaved document's own disk bytes are shadowed by its overlay, so the
                // change lands in a content source the analysis read from disk.
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

            match change {
                Change::None => {
                    assert_eq!(repeated.diagnostics(), first.diagnostics());
                    assert_eq!(
                        documents(&repeated, &cancellation),
                        documents(&first, &cancellation),
                        "a reused analysis realizes the same documents at the same routes"
                    );
                }
                Change::Opened | Change::Edited => {
                    let mut cold = SourceDiagnosticSession::new(Arc::clone(&config));
                    let cold_revision = cold.inspect(overlays, &cancellation).unwrap();
                    assert_eq!(
                        rendered(&repeated),
                        rendered(&cold_revision),
                        "a re-derived check answers what a cold check answers"
                    );
                }
                _ => {}
            }
        }
    }

    /// A revision's diagnostics in the shape a comparison can hold.
    fn rendered(revision: &SourceRevision) -> Vec<String> {
        revision
            .diagnostics()
            .iter()
            .map(|diagnostic| format!("{diagnostic:?}"))
            .collect()
    }

    /// Reuse answers a whole site exactly as a cold check does.
    ///
    /// A reused revision must not observe fewer sources than the revision it reuses: diagnostics,
    /// the documents realized and their routes all have to match the fresh derivation.
    #[test]
    fn reused_revision_matches_cold_check_over_many_sources() {
        let (directory, config) = site();
        for index in 0..50 {
            std::fs::write(
                directory.path().join(format!("content/doc-{index:02}.typ")),
                format!("= Document {index}\n\nBody {index}.\n"),
            )
            .unwrap();
        }
        let cancellation = BuildCancellation::new();
        let mut reused = SourceDiagnosticSession::new(Arc::clone(&config));
        reused.inspect(Vec::new(), &cancellation).unwrap();
        let revision = reused.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            reused.reused_source_analysis(),
            "an unchanged revision over fifty sources reuses its analysis"
        );

        let mut cold = SourceDiagnosticSession::new(Arc::clone(&config));
        let fresh = cold.inspect(Vec::new(), &cancellation).unwrap();
        assert_eq!(rendered(&revision), rendered(&fresh));
        assert_eq!(
            documents(&revision, &cancellation),
            documents(&fresh, &cancellation)
        );
    }

    /// The licence covers exactly what the analysis read, and nothing else is claimed.
    ///
    /// A configured asset tree no source reads is invisible to the source analysis, so changing a
    /// member of it does not refuse reuse — and cannot make an answer stale, because the site
    /// program (which is what reads configured assets) is realized again on every revision. A tree
    /// member the analysis does read appears in its read evidence and refuses reuse like any other
    /// changed file.
    #[test]
    fn unread_asset_tree_does_not_refuse_reuse() {
        use crate::config::section::assets::{AssetTreeDeclaration, AssetUrlPrefix};
        let (directory, config) = site();
        let mut config = config.as_ref().clone();
        let tree = directory.path().join("shared");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("member.txt"), "member").unwrap();
        std::fs::write(directory.path().join("content/post.typ"), "Body.\n").unwrap();
        config.assets.trees = vec![AssetTreeDeclaration::new(
            &tree,
            AssetUrlPrefix::parse("/shared").unwrap(),
        )];
        let config = Arc::new(config);
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&config));
        session.inspect(Vec::new(), &cancellation).unwrap();
        let tree = crate::filesystem::normalize_existing_prefix(&tree);
        assert!(
            !session
                .analysis
                .as_ref()
                .expect("a successful revision retains its evidence")
                .dependencies
                .physical_read_paths()
                .iter()
                .any(|path| path.starts_with(&tree)),
            "the source analysis read nothing under the declared asset tree"
        );

        std::fs::write(tree.join("member.txt"), "changed").unwrap();
        session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            session.reused_source_analysis(),
            "a tree the analysis never read does not refuse reuse"
        );
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
