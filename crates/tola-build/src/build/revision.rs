//! Complete build values, checked revision handoff, and recoverable tree replacement.

use anyhow::Result;

use crate::{config::ResolvedSiteConfig, hooks, site::SiteIndex};

use super::diagnostic;
use super::input::{input_observation, stale_input_error};
use crate::cancellation::BuildCancellation;

use super::{BuildCacheUpdate, BuildMode, InputFreshness, InputObservation};

/// One complete validated build, with its configuration, invocation, and inputs.
///
/// Custom callers may inspect it before [`write_site`] or prepare an immutable
/// revision. Ordinary repeated disk builds use [`super::BuildSession::build_and_write`].
pub struct SiteBuild {
    pub(super) config: std::sync::Arc<ResolvedSiteConfig>,
    pub(super) mode: BuildMode,
    pub(super) cancellation: BuildCancellation,
    pub(crate) index: SiteIndex,
    pub(crate) graph: crate::output::graph::OutputGraph,
    pub(super) cache_update: BuildCacheUpdate,
    pub(crate) diagnostics: Vec<tola_diagnostic::Diagnostic>,
    pub(crate) references: crate::site::references::References,
    pub(crate) source_hooks: crate::hooks::SourceHookOutputs,
    pub(super) report: Option<super::BuildReport>,
    /// Modules this build prepared; freshness and observations re-read the same manifests.
    pub(super) modules: tola_module::prepared::PreparedModules,
    /// The invocation's input scope, for after-publish hook environments.
    pub(super) scope: tola_input::InputScope,
}

/// An immutable revision that has not been checked against its inputs yet.
///
/// Construct this outside the host's event guard. The site, hook evidence, and
/// caches come from the same build; [`Self::check`] verifies their inputs.
pub struct UncheckedRevision {
    site: crate::site::SiteRevision,
    cancellation: BuildCancellation,
    source_hooks: crate::hooks::SourceHookOutputs,
    cache_update: BuildCacheUpdate,
    modules: tola_module::prepared::PreparedModules,
}

impl UncheckedRevision {
    pub fn site(&self) -> &crate::site::SiteRevision {
        &self.site
    }

    pub fn hook_outputs(&self) -> &[crate::hooks::HookOutputEvidence] {
        self.source_hooks.outputs()
    }

    pub fn source_hooks(&self) -> &crate::hooks::SourceHookOutputs {
        &self.source_hooks
    }

    pub fn freshness(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<InputFreshness> {
        self.cache_update.freshness(
            self.site.config(),
            &self.modules,
            self.source_hooks.outputs(),
            &self.cancellation,
            cancellation,
        )
    }

    /// Consume the revision and check its current inputs once. The host's event
    /// guard must still cover the interval between this check and replacement.
    pub fn check(self, cancellation: &BuildCancellation) -> Result<RevisionCheck> {
        let freshness = self.freshness(cancellation)?;
        if freshness.all_inputs_are_fresh() {
            Ok(RevisionCheck::Fresh(CheckedRevision {
                unchecked: Box::new(self),
            }))
        } else {
            Ok(RevisionCheck::Stale(freshness))
        }
    }
}

/// One unchecked revision whose inputs matched at a single point in time.
///
/// Only [`UncheckedRevision::check`] constructs this value. The host's event
/// guard must reject intervening changes before replacing its current revision.
/// Hand it to [`super::BuildSession::install_revision`] on the originating session.
pub struct CheckedRevision {
    unchecked: Box<UncheckedRevision>,
}

impl CheckedRevision {
    /// Keep cancellation and the cache handoff inside the originating session's gate.
    pub(super) fn into_parts(self) -> Result<(crate::site::SiteRevision, BuildCacheUpdate)> {
        let UncheckedRevision {
            site,
            cancellation,
            cache_update,
            ..
        } = *self.unchecked;
        cancellation.ensure_active()?;
        Ok((site, cache_update))
    }
}

impl std::fmt::Debug for CheckedRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckedRevision")
            .field("site", &self.unchecked.site)
            .finish()
    }
}

/// The result of one input check; a stale result yields no revision.
#[must_use]
#[derive(Debug)]
pub enum RevisionCheck {
    /// Inputs matched when checked; the host still guards the installation gap.
    Fresh(CheckedRevision),
    /// These input groups changed, so no revision can be handed off.
    Stale(InputFreshness),
}

impl RevisionCheck {
    /// Return the checked revision, or render the existing stale-input report.
    /// This conversion does not inspect the filesystem again.
    pub fn into_checked(self) -> Result<CheckedRevision> {
        match self {
            Self::Fresh(checked) => Ok(checked),
            Self::Stale(freshness) => Err(stale_input_error(&freshness)),
        }
    }
}

impl SiteBuild {
    /// Projections retained by [`super::BuildRequest::capture_report`].
    pub fn report(&self) -> Option<&super::BuildReport> {
        self.report.as_ref()
    }

    pub fn config(&self) -> &std::sync::Arc<ResolvedSiteConfig> {
        &self.config
    }

    /// The module directories selected by this build, keyed by their configured aliases.
    pub fn module_roots(&self) -> impl Iterator<Item = (&str, &std::path::Path)> {
        self.modules
            .modules
            .iter()
            .map(|(alias, module)| (alias.as_str(), module.root.as_path()))
    }

    /// Realized documents, resources, and addresses from this build.
    pub fn index(&self) -> &SiteIndex {
        &self.index
    }
    pub fn graph(&self) -> &crate::output::graph::OutputGraph {
        &self.graph
    }
    pub fn diagnostics(&self) -> &[tola_diagnostic::Diagnostic] {
        &self.diagnostics
    }
    pub fn references(&self) -> &crate::site::references::References {
        &self.references
    }
    pub fn hook_outputs(&self) -> &[crate::hooks::HookOutputEvidence] {
        self.source_hooks.outputs()
    }

    pub fn source_hooks(&self) -> &crate::hooks::SourceHookOutputs {
        &self.source_hooks
    }

    pub fn input_observation(&self) -> InputObservation {
        let caches = self.cache_update.caches();
        let mut observation = input_observation(
            caches.dependencies.physical_read_paths(),
            caches.dependencies.package_checks().to_vec(),
            Some(&caches.configured_assets),
            Some(caches.icons.evidence()),
            self.source_hooks.outputs(),
            caches.host.font_read_paths(),
        );
        crate::build::module_inputs::observe_manifests(&self.modules, &mut observation);
        observation.normalize_paths();
        observation
    }

    pub fn freshness(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<InputFreshness> {
        self.cache_update.freshness(
            &self.config,
            &self.modules,
            self.source_hooks.outputs(),
            &self.cancellation,
            cancellation,
        )
    }

    pub fn mode(&self) -> BuildMode {
        self.mode
    }

    /// Check this build using both the attempt's cancellation and the caller's.
    pub fn ensure_fresh(&self, cancellation: &BuildCancellation) -> Result<()> {
        let freshness = self.freshness(cancellation)?;
        if freshness.all_inputs_are_fresh() {
            Ok(())
        } else {
            Err(stale_input_error(&freshness))
        }
    }

    /// Prepare a complete immutable revision outside the host's event guard.
    /// `previous` allows unchanged output storage to be shared. It does not
    /// select or replace the host's current revision.
    pub fn into_unchecked_revision(
        self,
        previous: Option<&crate::output::revision::OutputRevision>,
    ) -> UncheckedRevision {
        let input_observation = self.input_observation();
        let outputs = match previous {
            Some(previous) => {
                crate::output::revision::OutputRevision::from_graph_reusing(previous, &self.graph)
            }
            None => crate::output::revision::OutputRevision::from_graph(&self.graph),
        };
        let mut diagnostics = crate::config::diagnostic::warning_diagnostics(&self.config);
        diagnostics.extend(self.diagnostics);
        UncheckedRevision {
            site: crate::site::SiteRevision {
                config: self.config,
                outputs,
                index: self.index,
                references: self.references,
                diagnostics,
                mode: self.mode,
                scope: self.scope,
                input_observation,
            },
            cancellation: self.cancellation,
            source_hooks: self.source_hooks,
            cache_update: self.cache_update,
            modules: self.modules,
        }
    }
}

/// A committed output tree and its optional, explicitly invoked consumers.
#[derive(Debug)]
pub struct WriteOutcome {
    output_root: std::path::PathBuf,
    counts: crate::output::summary::OutputCounts,
    diagnostics: Vec<tola_diagnostic::Diagnostic>,
    config: std::sync::Arc<ResolvedSiteConfig>,
    mode: BuildMode,
    scope: tola_input::InputScope,
    consumer_graph: Option<crate::output::graph::OutputGraph>,
}

impl WriteOutcome {
    pub fn output_root(&self) -> &std::path::Path {
        &self.output_root
    }

    pub fn counts(&self) -> crate::output::summary::OutputCounts {
        self.counts
    }

    /// Build diagnostics. Configuration warnings remain on the resolved configuration;
    /// its loader may already have presented them.
    pub fn diagnostics(&self) -> &[tola_diagnostic::Diagnostic] {
        &self.diagnostics
    }

    /// Consume the exact graph that was committed, independent of later writes.
    /// Release the site's build lock before invoking commands. Failure or
    /// cancellation here cannot undo the committed output.
    pub fn run_after_publish(
        self,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<tola_diagnostic::Diagnostic>> {
        if let Some(graph) = self.consumer_graph {
            let files = crate::output::files::HookOutputFiles::materialize_candidate(
                self.config.get_root(),
                &graph,
                Some(cancellation),
                crate::compiler::minified_languages(&self.config),
            )?;
            return hooks::run_after_publish_hooks(
                &self.config,
                self.mode,
                self.scope,
                files.root(),
                cancellation,
            );
        }
        Ok(Vec::new())
    }
}

/// Write an inspected build with recoverable output-tree replacement.
/// Recheck inputs and the original cancellation immediately before replacement.
/// No consumer runs here; invoke [`WriteOutcome::run_after_publish`] after releasing the build lock.
///
/// This acquires the site lock. A caller already holding it uses
/// [`super::SiteBuildLock::write_site`] or [`super::SiteBuildGuard::write_site`] instead.
///
/// For repeated builds with cache acceptance, use [`super::BuildSession::build_and_write`].
pub fn write_site(build: &SiteBuild) -> Result<WriteOutcome> {
    let lock = super::SiteBuildLock::acquire(&build.config, &build.cancellation, || {})
        .map_err(|error| write_error(build, anyhow::Error::new(error)))?;
    write_site_locked(build, &lock)
}

pub(super) fn write_site_locked(
    build: &SiteBuild,
    lock: &super::SiteBuildLock,
) -> Result<WriteOutcome> {
    write_site_inner(build, lock).map_err(|error| write_error(build, error))
}

fn write_error(build: &SiteBuild, error: anyhow::Error) -> anyhow::Error {
    let error = diagnostic::with_write(error, build.config.get_root());
    if crate::cancellation::is_cancelled(&error) || build.diagnostics.is_empty() {
        return error;
    }
    // The write failure leads; the candidate build's warnings follow it so both stay visible.
    let mut diagnostics = tola_diagnostic::attached(&error)
        .expect("a write failure carries diagnostics")
        .to_vec();
    diagnostics.extend(build.diagnostics.iter().cloned());
    tola_diagnostic::DiagnosticError::attach(error, diagnostics).into()
}

fn write_site_inner(build: &SiteBuild, lock: &super::SiteBuildLock) -> Result<WriteOutcome> {
    build.cancellation.ensure_active()?;
    let config = build.config.as_ref();
    let mode = build.mode();
    let graph = &build.graph;
    let caches = build.cache_update.caches();
    let physical_reads = caches.dependencies.physical_read_paths();
    let destination = crate::output::resolve_site_output_root_with_inputs(
        config,
        physical_reads
            .iter()
            .map(|path| ("file read by the Bundle", path.as_path())),
    )?;
    let mut writing = destination.write(lock, &build.cancellation)?;
    writing.minify = crate::compiler::minified_languages(config);
    let staged =
        crate::output::files::StagedBuildFiles::materialize(graph, &writing, &build.cancellation)?;
    build.ensure_fresh(&build.cancellation)?;
    staged.commit(writing)?;
    Ok(WriteOutcome {
        output_root: destination.path().to_path_buf(),
        counts: graph.counts(),
        diagnostics: build.diagnostics.clone(),
        config: std::sync::Arc::clone(&build.config),
        mode,
        scope: build.scope,
        consumer_graph: hooks::has_after_publish_hooks(config, mode).then(|| graph.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::pipeline::BuildAttemptProducers;
    use crate::build::tests::*;
    use crate::build::*;

    use std::fs;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn retargeted_module_root_is_rejected() {
        use std::os::unix::fs::symlink;
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        let source = "[build]\nentry=\"site.typ\"\n";
        for name in ["first", "second"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::write(root.join(name).join("tola-module.toml"), source).unwrap();
            fs::write(
                root.join(name).join("site.typ"),
                "#document(\"index.html\")[Site]",
            )
            .unwrap();
        }
        symlink("first", root.join("theme")).unwrap();
        let config = crate::config::tests::load_test_config(
            root,
            "[modules.forest]\npath=\"theme\"\n[build]\nentry={module=\"forest\"}\n",
        );
        let candidate = build_site(&config, BuildMode::Production).unwrap();
        fs::remove_file(root.join("theme")).unwrap();
        symlink("second", root.join("theme")).unwrap();
        assert!(
            candidate
                .freshness(&BuildCancellation::default())
                .unwrap()
                .is_stale(InputKind::ModuleManifests)
        );
        assert!(write_site(&candidate).is_err());
        assert!(!config.build.publish_dir.exists());
    }

    #[test]
    fn overlapping_bundle_read_stops_the_build() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let templates = root.join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(templates.join("value.txt"), "protected input").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            "#document(\"index.html\")[#read(\"/templates/value.txt\")]",
        )
        .unwrap();

        let mut config = site_config(root);
        config.build.publish_dir = config.get_root().join("templates");

        let error = match build_site(&config, BuildMode::Production) {
            Ok(_) => panic!("a Bundle read inside the published output compiled"),
            Err(error) => error,
        };

        // The read boundary refuses the output tree while the Bundle compiles, so publication
        // never reaches the write-stage overlap check that `output/root.rs` exercises directly.
        let diagnostics = error.diagnostics(config.get_root());
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "typst.compile"
                    && diagnostic.message.contains("templates/value.txt")
            }),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn unreported_change_forces_retry() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let template = root.join("template.typ");
        fs::write(&template, "#let message = \"first\"").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            "#import \"template.typ\": message\n#document(\"index.html\")[#message]",
        )
        .unwrap();

        let config = site_config(root);

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        install_build(&mut session, first);

        fs::write(&template, "#let message = \"second\"").unwrap();
        let decision = session.rebuild_decision(&[], false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        assert!(matches!(
            &reuse,
            crate::compiler::CompilationReuse::SiteProgram { .. }
        ));
        let stale = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        assert!(
            !stale
                .cache_update
                .caches()
                .dependencies
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );

        let changed_reads = stale
            .cache_update
            .caches()
            .dependencies
            .physical_read_paths();
        let retry_decision = session.rebuild_decision(&changed_reads, false);
        let retry_reuse = session.reusable_compilation(&retry_decision, true).unwrap();
        assert!(matches!(
            &retry_reuse,
            crate::compiler::CompilationReuse::SourceAnalysis { .. }
        ));
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(retry_reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let graph = second.graph;
        let html = String::from_utf8_lossy(output_bytes(&graph, "index.html"));
        assert!(html.contains("second"), "{html}");

        let fresh_host = compiler_host(&config).unwrap();
        let fresh = build_site_with_host(
            &config,
            &fresh_host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        let fresh_graph = fresh.graph;
        assert_eq!(
            output_bytes(&graph, "index.html"),
            output_bytes(&fresh_graph, "index.html"),
            "the write retry must match a clean compilation",
        );
    }
}
