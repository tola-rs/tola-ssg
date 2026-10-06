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
    pub(crate) diagnostics: Vec<crate::diagnostic::Diagnostic>,
    pub(crate) references: crate::site::references::References,
    pub(crate) source_hooks: crate::hooks::SourceHookOutputs,
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
    pub fn config(&self) -> &std::sync::Arc<ResolvedSiteConfig> {
        &self.config
    }

    /// Realized documents, resources, and addresses from this build.
    pub fn index(&self) -> &SiteIndex {
        &self.index
    }
    pub fn graph(&self) -> &crate::output::graph::OutputGraph {
        &self.graph
    }
    pub fn diagnostics(&self) -> &[crate::diagnostic::Diagnostic] {
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
        input_observation(
            caches.dependencies.physical_read_paths(),
            caches.dependencies.package_checks().to_vec(),
            Some(&caches.configured_assets),
            Some(caches.icons.evidence()),
            self.source_hooks.outputs(),
            caches.host.font_read_paths(),
        )
    }

    pub fn freshness(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<InputFreshness> {
        self.cache_update.freshness(
            &self.config,
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
            site: crate::site::SiteRevision::new(
                self.config,
                outputs,
                self.index,
                self.references,
                diagnostics,
                self.mode,
                input_observation,
            ),
            cancellation: self.cancellation,
            source_hooks: self.source_hooks,
            cache_update: self.cache_update,
        }
    }
}

/// A committed output tree and its optional, explicitly invoked consumers.
#[derive(Debug)]
pub struct WriteOutcome {
    output_root: std::path::PathBuf,
    counts: crate::output::summary::OutputCounts,
    diagnostics: Vec<crate::diagnostic::Diagnostic>,
    config: std::sync::Arc<ResolvedSiteConfig>,
    mode: BuildMode,
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
    pub fn diagnostics(&self) -> &[crate::diagnostic::Diagnostic] {
        &self.diagnostics
    }

    /// Consume the exact graph that was committed, independent of later writes.
    /// Release the site's build lock before invoking commands. Failure or
    /// cancellation here cannot undo the committed output.
    pub fn run_after_publish(self, cancellation: &BuildCancellation) -> Result<()> {
        if let Some(graph) = self.consumer_graph {
            let files = crate::output::files::HookOutputFiles::materialize_candidate(
                self.config.get_root(),
                &graph,
                Some(cancellation),
            )?;
            hooks::run_after_publish_hooks(&self.config, self.mode, files.root(), cancellation)?;
        }
        Ok(())
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
        .map_err(|error| diagnostic::with_write(error, build.config.get_root()))?;
    write_site_locked(build, &lock)
}

pub(super) fn write_site_locked(
    build: &SiteBuild,
    lock: &super::SiteBuildLock,
) -> Result<WriteOutcome> {
    write_site_inner(build, lock)
        .map_err(|error| diagnostic::with_write(error, build.config.get_root()))
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
    let writing = destination.write(lock, &build.cancellation)?;
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
        consumer_graph: hooks::has_after_publish_hooks(config, mode).then(|| graph.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::pipeline::BuildAttemptProducers;
    use crate::build::tests::*;
    use crate::build::*;
    use crate::config::section::build::AfterPublishHookConfig;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn write_uses_candidate_configuration() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::write(
            root.join("site.typ"),
            "#document(\"index.html\")[Bound site]",
        )
        .unwrap();
        let mut config = site_config(root);
        let destination = config.build.publish_dir.clone();
        let candidate = build_site(&config, BuildMode::Production).unwrap();
        config.build.publish_dir = config.get_root().join("another-write");

        assert_eq!(candidate.config().build().publish_dir, destination);
        candidate
            .ensure_fresh(&BuildCancellation::default())
            .unwrap();
        write_site(&candidate).unwrap();

        assert!(destination.join("index.html").is_file());
        assert!(!config.build.publish_dir.exists());
    }

    #[test]
    fn complete_revision_keeps_write_evidence() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::write(root.join("value.txt"), "Original value").unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#document("index.html")[#read("value.txt")]"#,
        )
        .unwrap();
        let config = site_config(root);
        let candidate = build_site(&config, BuildMode::Development).unwrap();
        let bound_config = std::sync::Arc::clone(candidate.config());
        let write = candidate.into_unchecked_revision(None);

        assert!(std::sync::Arc::ptr_eq(write.site().config(), &bound_config));
        let cancellation = crate::cancellation::BuildCancellation::default();
        assert!(
            write
                .freshness(&cancellation)
                .unwrap()
                .all_inputs_are_fresh()
        );
        fs::write(
            root.join("value.txt"),
            "Changed before the output is written",
        )
        .unwrap();
        assert!(
            write
                .freshness(&cancellation)
                .unwrap()
                .is_stale(InputKind::TypstPhysicalReads)
        );
    }

    #[test]
    fn freshness_checks_candidate_icon_sources() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        let icons = root.join("icons.json");
        fs::write(&entry, "#document(\"index.html\")[Ready]").unwrap();
        fs::write(
            &icons,
            r#"{"prefix":"brand","icons":{"mark":{"body":"<path d='M0 0h16v16H0z'/>"}}}"#,
        )
        .unwrap();
        let mut config = site_config(root);
        config.icons.collections.insert(
            "brand".into(),
            crate::config::section::IconCollectionSource::LocalJson {
                path: icons.clone(),
            },
        );
        let candidate = build_site(&config, BuildMode::Production).unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let freshness = || candidate.freshness(&cancellation);
        assert!(freshness().unwrap().all_inputs_are_fresh());

        fs::write(
            &icons,
            r#"{"prefix":"brand","icons":{"mark":{"body":"<path d='M0 0h12v12H0z'/>"}}}"#,
        )
        .unwrap();
        assert!(freshness().unwrap().is_stale(InputKind::Icons));

        canceller.cancel();
        assert!(crate::cancellation::is_cancelled(&freshness().unwrap_err()));
    }

    #[test]
    fn stale_typst_reads_stop_the_write() {
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

        let candidate = build_site(&config, BuildMode::Production).unwrap();
        fs::write(&template, "#let message = \"second\"").unwrap();

        let error = write_site(&candidate).unwrap_err();
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics[0].code, "write.stale");
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
        let diagnostics = error_diagnostics(&error, config.get_root());
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "typst.compile"
                    && diagnostic.message.contains("templates/value.txt")
            }),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn consumer_failure_keeps_publication() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "#document(\"index.html\")[Published]").unwrap();

        let mut config = site_config(root);
        config
            .build
            .hooks
            .after_publish
            .push(AfterPublishHookConfig {
                name: "failing-consumer".into(),
                command: hook_child_command("build::revision::tests::fails_after_publish"),
                ..AfterPublishHookConfig::default()
            });

        let mut session = BuildSession::new();
        let written = session
            .build_and_write(&config, BuildRequest::default())
            .unwrap();
        assert_eq!(written.counts().pages, 1);
        assert!(
            written
                .run_after_publish(&BuildCancellation::default())
                .is_err()
        );
        assert!(config.build.publish_dir.join("index.html").is_file());
        let decision = session.rebuild_decision(&[], false);
        assert!(session.reusable_compilation(&decision, true).is_some());
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

    #[test]
    fn consumer_keeps_committed_revision() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "#document(\"index.html\")[First publication]").unwrap();
        let mut config = site_config(root);
        config
            .build
            .hooks
            .after_publish
            .push(AfterPublishHookConfig {
                name: "capture-output".into(),
                command: hook_child_command("build::revision::tests::copies_after_publish_output"),
                ..AfterPublishHookConfig::default()
            });
        let first = build_site(&config, BuildMode::Production).unwrap();
        let expected = output_bytes(first.graph(), "index.html").to_vec();
        let published = write_site(&first).unwrap();
        assert!(!root.join("consumed.html").exists());

        fs::write(&entry, "#document(\"index.html\")[Second publication]").unwrap();
        config.build.hooks.after_publish[0].command =
            hook_child_command("build::revision::tests::fails_after_publish");
        let second = build_site(&config, BuildMode::Production).unwrap();
        write_site(&second).unwrap();
        published
            .run_after_publish(&BuildCancellation::default())
            .unwrap();

        assert_eq!(fs::read(root.join("consumed.html")).unwrap(), expected);
        assert_eq!(
            fs::read(config.build.publish_dir.join("index.html")).unwrap(),
            output_bytes(second.graph(), "index.html")
        );
    }

    #[test]
    fn copies_after_publish_output() {
        if !is_hook_child_for("after-publish") {
            return;
        }
        let input = std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_INPUT_DIR").unwrap());
        fs::copy(input.join("index.html"), "consumed.html").unwrap();
    }

    #[test]
    fn fails_after_publish() {
        if !is_hook_child_for("after-publish") {
            return;
        }
        panic!("intentional after-publish failure");
    }
}
