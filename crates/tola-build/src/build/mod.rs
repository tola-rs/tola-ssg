//! Construction, validation, and safe writing of one complete site candidate,
//! with the accepted caches and cross-process locks repeated builds need.

mod bootstrap;
mod diagnostic;
mod input;
mod lock;
pub(crate) mod module_inputs;
mod pipeline;
mod report;
mod revision;
mod session;

pub use crate::hooks::HookExecution;
pub use crate::mode::BuildMode;
pub use crate::observation::{InputKind, InputObservation, InputPathScope, ObservedInputPath};
pub use diagnostic::{for_error as error_diagnostics, no_pages_diagnostic};
pub(crate) use input::BuildAttemptInputs;
pub use input::{BuildFailure, InputFreshness, configured_input_observation};
pub use lock::{SiteBuildGuard, SiteBuildLock, SiteBuildLockError};
pub use pipeline::build_site;
pub use report::BuildReport;
pub use revision::{
    CheckedRevision, RevisionCheck, SiteBuild, UncheckedRevision, WriteOutcome, write_site,
};
/// Whether a build attempt checks the site's internal references.
///
/// Vendor verification skips it: references depend on outputs its build does not produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferenceChecks {
    /// Report a page that links to something the site does not publish.
    #[default]
    Full,
    /// Skip the reference check and report no reference diagnostics.
    Skip,
}

pub use session::{BuildAttempt, BuildRequest, BuildReuse, BuildSession, BuildTrigger};
pub(crate) use session::{BuildCacheUpdate, RetainedProducerCaches};

#[cfg(test)]
pub(crate) mod tests {
    //! Shared setup for build-stage and hook tests.

    use anyhow::Result;

    use crate::config::ResolvedSiteConfig;

    use super::pipeline::{BuildAttemptProducers, build_site_inner};
    use super::{
        BuildMode, BuildRequest, BuildSession, CheckedRevision, SiteBuild, UncheckedRevision,
        build_site, write_site,
    };

    pub(super) use crate::compiler::tests::compiler_host;

    /// Arguments that re-run one test as a source-generator hook child process.
    pub(crate) fn hook_child_command(test_name: &str) -> Vec<String> {
        vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--exact".into(),
            test_name.into(),
            "--nocapture".into(),
        ]
    }

    pub(crate) fn hook_context() -> Option<crate::context::Context> {
        crate::context::Context::read().unwrap()
    }

    pub(crate) fn is_hook_child_for(stage: &str) -> bool {
        hook_context()
            .and_then(|context| context.hook)
            .is_some_and(|hook| hook.stage == stage)
    }

    const RECORDED_CACHE_DIRECTORY: &str = "hook-cache-directory.txt";
    const RECORDED_BUILD_MODE: &str = "hook-build-mode.txt";

    pub(crate) fn assert_hook_environment(stage: &str) {
        let hook = hook_context().unwrap().hook.unwrap();
        assert_eq!(hook.stage, stage);
        for name in ["TMPDIR", "TMP", "TEMP"] {
            assert_eq!(
                std::env::var_os(name).as_deref(),
                Some(hook.temp_dir.as_os_str())
            );
        }
        let root = std::env::current_dir().unwrap();
        let expected_parent = tola_input::layout::hook_scratch(&root)
            .canonicalize()
            .unwrap();
        assert!(
            hook.temp_dir
                .canonicalize()
                .unwrap()
                .starts_with(expected_parent)
        );
        let cache = hook.cache_dir.unwrap();
        assert!(cache.is_absolute() && cache.is_dir());
        assert_eq!(hook.input_dir.is_some(), stage != "before-build");
        assert_eq!(hook.output_dir.is_some(), stage == "generate-outputs");
        for directory in [hook.input_dir, hook.output_dir].into_iter().flatten() {
            assert!(directory.is_absolute() && directory.is_dir());
        }
    }

    pub(crate) fn record_hook_environment() {
        let hook = hook_context().unwrap().hook.unwrap();
        let cache = hook.cache_dir.unwrap();
        std::fs::write(RECORDED_CACHE_DIRECTORY, cache.to_string_lossy().as_bytes()).unwrap();
        std::fs::write(RECORDED_BUILD_MODE, hook.mode.unwrap()).unwrap();
        let log = cache.join("invocations.txt");
        let runs = std::fs::read_to_string(&log).unwrap_or_default();
        std::fs::write(log, format!("{runs}run\n")).unwrap();
    }

    /// Install a built site's caches as the session's accepted baseline.
    pub(super) fn accept_build(session: &mut BuildSession, build: SiteBuild) {
        let checked = check_fresh(build.into_unchecked_revision(None));
        session.install_revision(checked, |_| Ok(())).unwrap();
    }

    /// Re-executed by hook environment tests as the command's child process.
    #[test]
    fn child_records_hook_environment() {
        if hook_context().is_none() {
            return;
        }
        record_hook_environment();
    }

    /// The cache directory the last recorded hook run reported.
    pub(crate) fn recorded_hook_cache_directory(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::read_to_string(root.join(RECORDED_CACHE_DIRECTORY))
            .unwrap()
            .into()
    }

    /// The build mode the last recorded hook run reported.
    pub(crate) fn recorded_build_mode(root: &std::path::Path) -> String {
        std::fs::read_to_string(root.join(RECORDED_BUILD_MODE)).unwrap()
    }

    /// Check one unchecked revision that is known to match its inputs.
    pub(super) fn check_fresh(unchecked: UncheckedRevision) -> CheckedRevision {
        unchecked
            .check(&crate::cancellation::BuildCancellation::default())
            .unwrap()
            .into_checked()
            .unwrap()
    }

    /// Install a candidate built outside `session`, attributing it to that session.
    ///
    /// [`build_site_with_host`] bypasses `BuildAttempt::run`, which is what normally
    /// records a candidate's originating session. Handing such a candidate straight to
    /// [`BuildSession::install_revision`] would be rejected as a foreign candidate, so
    /// this helper records the same attribution a prepared attempt would.
    pub(super) fn install_build(session: &mut BuildSession, mut build: SiteBuild) {
        build.cache_update.attribute_to(session);
        let checked = check_fresh(build.into_unchecked_revision(None));
        session.install_revision(checked, |_| Ok(())).unwrap();
    }

    pub(super) fn build_site_with_host(
        config: &ResolvedSiteConfig,
        host: &crate::compiler::SiteHost,
        mode: BuildMode,
        producers: &mut BuildAttemptProducers,
    ) -> Result<SiteBuild> {
        let bundle = producers.cancellation.bundle_cancellation();
        let site = crate::modules::prepared::prepare_site_with_bundle(
            config,
            producers.resources.input_mode(),
            &bundle,
        )?;
        let request = BuildRequest {
            mode,
            ..Default::default()
        };
        build_site_inner(&site, &request, producers, |producers, _| {
            if host.matches(site.config(), &producers.resources)
                && host.font_inventory_is_fresh(&producers.cancellation)?
            {
                Ok(host.clone())
            } else {
                crate::compiler::SiteHost::for_config(
                    site.config(),
                    &producers.resources,
                    std::sync::Arc::clone(site.local_packages()),
                    &producers.cancellation,
                )
                .map_err(anyhow::Error::new)
            }
        })
    }

    /// Load a resolved config for the conventional `site.typ`/`content` layout
    /// under `root`.
    pub(super) fn site_config(root: &std::path::Path) -> ResolvedSiteConfig {
        site_config_from(root, "")
    }

    /// Like [`site_config`], with `source` as the site's configuration document.
    pub(super) fn site_config_from(root: &std::path::Path, source: &str) -> ResolvedSiteConfig {
        std::fs::create_dir_all(root.join("content")).unwrap();
        let mut config = crate::config::tests::load_test_config(root, source);
        config.build.entry = root.join("site.typ");
        config.build.content_dir = config.get_root().join("content");
        config
    }

    pub(super) fn build_and_publish(config: &ResolvedSiteConfig) -> Result<SiteBuild> {
        let build = build_site(config, BuildMode::Production)?;
        write_site(&build)?;
        Ok(build)
    }

    pub(super) fn output_bytes<'a>(
        graph: &'a crate::output::graph::OutputGraph,
        path: &str,
    ) -> &'a [u8] {
        graph
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == path)
            .unwrap_or_else(|| panic!("missing output {path}"))
            .bytes()
    }

    pub(super) fn has_output(graph: &crate::output::graph::OutputGraph, path: &str) -> bool {
        graph
            .outputs()
            .iter()
            .any(|output| output.path().as_str() == path)
    }
}
