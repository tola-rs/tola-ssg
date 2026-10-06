//! Construction, validation, and safe writing of one complete site candidate,
//! with the accepted caches and cross-process locks repeated builds need.

mod diagnostic;
mod input;
mod lock;
mod pipeline;
mod revision;
mod session;

pub use crate::hooks::HookExecution;
pub use crate::mode::BuildMode;
pub use crate::observation::{InputKind, InputObservation, InputScope, ObservedInputPath};
pub use diagnostic::{for_error as error_diagnostics, no_pages_diagnostic};
pub(crate) use input::{AcceptedFileChanges, BuildAttemptInputs};
pub use input::{BuildFailure, InputFreshness, configured_input_observation};
pub use lock::{SiteBuildGuard, SiteBuildLock};
pub use pipeline::build_site;
pub use revision::{
    CheckedRevision, RevisionCheck, SiteBuild, UncheckedRevision, WriteOutcome, write_site,
};
pub use session::{BuildAttempt, BuildRequest, BuildReuse, BuildSession, BuildTrigger};
pub(crate) use session::{BuildCacheUpdate, RetainedProducerCaches};

#[cfg(test)]
pub(crate) mod tests {
    //! Shared setup for build-stage and hook tests.

    use anyhow::Result;

    use crate::config::ResolvedSiteConfig;

    use super::pipeline::{BuildAttemptProducers, build_site_inner};
    use super::{
        BuildMode, BuildSession, CheckedRevision, SiteBuild, UncheckedRevision, build_site,
        write_site,
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

    pub(crate) fn is_hook_child_for(stage: &str) -> bool {
        std::env::var_os("TOLA_HOOK_STAGE").as_deref() == Some(std::ffi::OsStr::new(stage))
    }

    /// The file names a hook environment record is read back from, below the site root.
    const RECORDED_CACHE_DIRECTORY: &str = "hook-cache-directory.txt";
    const RECORDED_BUILD_MODE: &str = "hook-build-mode.txt";
    const RECORDED_TEMPORARY_DIRECTORY: &str = "hook-temporary-directory.txt";

    /// Assert the environment Tola supplies to one hook command child process.
    ///
    /// The caller names the stage it configured, so a wrong stage or a leaked directory
    /// variable fails here rather than passing as an unread value.
    pub(crate) fn assert_hook_environment(stage: &str) {
        assert_eq!(
            std::env::var_os("TOLA_HOOK_STAGE").as_deref(),
            Some(std::ffi::OsStr::new(stage))
        );
        let temporary = std::env::var_os("TMPDIR").expect("a hook child receives TMPDIR");
        assert_eq!(
            std::env::var_os("TMP").as_deref(),
            Some(temporary.as_os_str())
        );
        assert_eq!(
            std::env::var_os("TEMP").as_deref(),
            Some(temporary.as_os_str())
        );
        assert_eq!(
            std::env::var_os("TOLA_HOOK_TEMP_DIR").as_deref(),
            Some(temporary.as_os_str())
        );
        let root = std::env::current_dir().unwrap();
        let temporary = std::fs::canonicalize(temporary).unwrap();
        let expected_parent = std::fs::canonicalize(root.join(".tola/hook-tmp")).unwrap();
        assert!(temporary.starts_with(expected_parent));
        let cache = std::env::var_os("TOLA_HOOK_CACHE_DIR")
            .expect("a hook child receives a cache directory");
        assert!(std::path::Path::new(&cache).is_absolute());
        assert!(std::path::Path::new(&cache).is_dir());
        for (name, applicable) in [
            ("TOLA_HOOK_INPUT_DIR", stage != "before-build"),
            ("TOLA_HOOK_OUTPUT_DIR", stage == "generate-outputs"),
        ] {
            match (std::env::var_os(name), applicable) {
                (Some(value), true) => assert!(
                    std::path::Path::new(&value).is_absolute()
                        && std::path::Path::new(&value).is_dir()
                ),
                (Some(value), false) => panic!(
                    "the {stage} child received `{name}` = {value:?}, which its stage does not define"
                ),
                (None, true) => panic!("the {stage} child received no `{name}`"),
                (None, false) => {}
            }
        }
    }

    /// Record the hook environment Tola supplied beside the site root, and note the run in
    /// the cache directory, so a parent test can compare runs of one identity.
    pub(crate) fn record_hook_environment() {
        let cache = std::env::var_os("TOLA_HOOK_CACHE_DIR")
            .expect("a hook child receives a cache directory");
        let mode = std::env::var_os("TOLA_BUILD_MODE").expect("a hook child receives a build mode");
        let temporary = std::env::var_os("TMPDIR").expect("a hook child receives TMPDIR");
        std::fs::write(RECORDED_CACHE_DIRECTORY, cache.to_string_lossy().as_bytes()).unwrap();
        std::fs::write(RECORDED_BUILD_MODE, mode.to_string_lossy().as_bytes()).unwrap();
        std::fs::write(
            RECORDED_TEMPORARY_DIRECTORY,
            temporary.to_string_lossy().as_bytes(),
        )
        .unwrap();
        let log = std::path::Path::new(&cache).join("invocations.txt");
        let runs = std::fs::read_to_string(&log).unwrap_or_default();
        std::fs::write(log, format!("{runs}run\n")).unwrap();
    }

    /// Re-executed by hook environment tests as the command's child process.
    #[test]
    fn records_hook_environment() {
        if std::env::var_os("TOLA_HOOK_CACHE_DIR").is_none() {
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

    /// The temporary directory the last recorded hook run reported.
    pub(crate) fn recorded_temporary_directory(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::read_to_string(root.join(RECORDED_TEMPORARY_DIRECTORY))
            .unwrap()
            .into()
    }

    /// Check one unchecked revision that is known to match its inputs.
    pub(super) fn check_fresh(unchecked: UncheckedRevision) -> CheckedRevision {
        unchecked
            .check(&crate::cancellation::BuildCancellation::default())
            .unwrap()
            .into_checked()
            .unwrap()
    }

    /// Install a built site's caches as the session's accepted baseline.
    pub(super) fn accept_build(session: &mut BuildSession, build: SiteBuild) {
        let checked = check_fresh(build.into_unchecked_revision(None));
        session.install_revision(checked, |_| Ok(())).unwrap();
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
        host: &crate::compiler::TypstHost,
        mode: BuildMode,
        producers: &mut BuildAttemptProducers,
    ) -> Result<SiteBuild> {
        build_site_inner(
            config,
            mode,
            crate::hooks::HookExecution::Run,
            producers,
            |producers, _| {
                if host.matches(config, &producers.resources)
                    && host.font_inventory_is_fresh(&producers.cancellation)?
                {
                    Ok(host.clone())
                } else {
                    crate::compiler::TypstHost::for_config_with_resources(
                        config,
                        &producers.resources,
                        &producers.cancellation,
                    )
                    .map_err(anyhow::Error::new)
                }
            },
        )
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
