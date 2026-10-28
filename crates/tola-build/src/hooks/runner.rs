//! Runs trusted hook commands at their configured build stages.

use super::command::{HookCall, HookDirectories, HookInvocation};
use super::evidence::{
    FailedHookOutputs, HookOutputEvidence, SourceHookOutputs, declared_output_files,
    validate_declared_output_boundaries,
};
use super::report::{hook_command_error, run_hook_entry};

use crate::cancellation::OptionalCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::BeforeBuildHookConfig;
use crate::config::section::build::hooks::HookStage;
use crate::mode::BuildMode;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Successful source generation completed before discovery.
#[derive(Debug)]
pub(crate) struct BeforeBuildExecution {
    source_hooks: SourceHookOutputs,
    hooks_executed: bool,
}

impl BeforeBuildExecution {
    pub(crate) fn empty() -> Self {
        Self {
            source_hooks: SourceHookOutputs::default(),
            hooks_executed: false,
        }
    }

    pub(crate) fn hooks_executed(&self) -> bool {
        self.hooks_executed
    }

    pub(crate) fn into_source_hooks(self) -> SourceHookOutputs {
        self.source_hooks
    }
}

#[derive(Debug)]
pub(crate) struct SourceHookFailure {
    error: anyhow::Error,
    completed: SourceHookOutputs,
    partial: Option<FailedHookOutputs>,
}

impl SourceHookFailure {
    fn new(
        mut error: anyhow::Error,
        completed: SourceHookOutputs,
        paths: &BTreeSet<PathBuf>,
        cancellation: Option<&crate::cancellation::BuildCancellation>,
    ) -> Self {
        let partial = if error.is::<crate::cancellation::BuildCancelled>() {
            None
        } else {
            match FailedHookOutputs::capture(paths, cancellation) {
                Ok(partial) => partial,
                Err(cancelled) => {
                    error = cancelled;
                    None
                }
            }
        };
        Self {
            error,
            completed,
            partial,
        }
    }

    pub(crate) fn completed(&self) -> &SourceHookOutputs {
        &self.completed
    }

    pub(crate) fn partial_outputs(&self) -> Option<&FailedHookOutputs> {
        self.partial.as_ref()
    }
}

impl std::fmt::Display for SourceHookFailure {
    /// The failing stage already wrote the sentence its reader needs, including the
    /// bounded command output; a failure boundary renders only this outermost text.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.error)
    }
}

impl std::error::Error for SourceHookFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

fn run_before_build_hook(
    hook: &BeforeBuildHookConfig,
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    cancellation: Option<&crate::cancellation::BuildCancellation>,
    executed_outputs: &mut BTreeSet<PathBuf>,
) -> Result<Vec<HookOutputEvidence>> {
    cancellation.ensure_active_if_present()?;
    run_hook_entry(HookStage::BeforeBuild, &hook.name, || {
        let before = validate_declared_output_boundaries(hook, config, &config.build.publish_dir)?;
        executed_outputs.extend(
            before
                .iter()
                .map(|identity| identity.logical_path().to_path_buf()),
        );
        let invocation = HookInvocation::new(
            &hook.command,
            HookCall {
                site_root: config.get_root(),
                name: &hook.name,
                mode,
                directories: HookDirectories::BeforeBuild,
            },
        )?;
        let completion = invocation.run(cancellation);
        cancellation.ensure_active_if_present()?;
        completion.map_err(|error| {
            hook_command_error(
                HookStage::BeforeBuild,
                &super::hook_identity(HookStage::BeforeBuild, &hook.name),
                error,
            )
        })?;
        let after = declared_output_files(hook, config, cancellation)?;
        for files in &after {
            if files.entries.is_empty() {
                let declared = crate::filesystem::display_path(
                    files.source_identity.logical_path(),
                    config.get_root(),
                );
                return Err(super::hook_contract_error(
                    format!(
                        "{} declares `{declared}`, but the hook wrote nothing",
                        super::hook_identity(HookStage::BeforeBuild, &hook.name)
                    ),
                    "write it, or remove it from `outputs`",
                ));
            }
        }
        after
            .into_iter()
            .map(|files| HookOutputEvidence::from_successful_output(files, cancellation))
            .collect()
    })
}

pub(crate) fn run_before_build_hooks_with_cancellation(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    hook_execution: crate::hooks::HookExecution,
    cancellation: Option<&crate::cancellation::BuildCancellation>,
) -> Result<BeforeBuildExecution, SourceHookFailure> {
    if !hook_execution.runs() {
        return Ok(BeforeBuildExecution::empty());
    }
    let mut completed = SourceHookOutputs::default();
    let mut hooks_executed = false;
    if let Err(error) = cancellation.ensure_active_if_present() {
        return Err(SourceHookFailure::new(
            error.into(),
            completed,
            &BTreeSet::new(),
            None,
        ));
    }
    for hook in config.build.hooks.before_build.iter() {
        if !super::hook_participates(hook.enable, hook.dev, mode) {
            continue;
        }
        let mut started_outputs = BTreeSet::new();
        let outputs = run_before_build_hook(hook, config, mode, cancellation, &mut started_outputs);
        // A hook contributes successful evidence atomically; failure observations
        // remain separate even when earlier declarations were readable.
        match outputs {
            Ok(outputs) => completed.outputs.extend(outputs),
            Err(error) => {
                return Err(SourceHookFailure::new(
                    error,
                    completed,
                    &started_outputs,
                    cancellation,
                ));
            }
        }
        hooks_executed = true;
    }
    if let Err(error) = cancellation.ensure_active_if_present() {
        return Err(SourceHookFailure::new(
            error.into(),
            completed,
            &BTreeSet::new(),
            None,
        ));
    }
    Ok(BeforeBuildExecution {
        source_hooks: completed,
        hooks_executed,
    })
}

pub fn has_after_publish_hooks(config: &ResolvedSiteConfig, mode: BuildMode) -> bool {
    config
        .build
        .hooks
        .after_publish
        .iter()
        .any(|hook| super::hook_participates(hook.enable, hook.dev, mode))
}

/// Consume one committed output view using every participating hook.
///
/// Cancellation stops the command chain without undoing its external effects.
/// Hosts supply their consumer-lifetime observer, independently of cancellation
/// for superseded build attempts.
pub fn run_after_publish_hooks(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    output_root: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<()> {
    for hook in config.build.hooks.after_publish.iter() {
        cancellation.ensure_active()?;
        if super::hook_participates(hook.enable, hook.dev, mode) {
            run_hook_entry(HookStage::AfterPublish, &hook.name, || {
                let invocation = HookInvocation::new(
                    &hook.command,
                    HookCall {
                        site_root: config.get_root(),
                        name: &hook.name,
                        mode,
                        directories: HookDirectories::AfterPublish { input: output_root },
                    },
                )?;
                let completion = invocation.run(Some(cancellation));
                completion.map(|_output| ()).map_err(|error| {
                    hook_command_error(
                        HookStage::AfterPublish,
                        &super::hook_identity(HookStage::AfterPublish, &hook.name),
                        error,
                    )
                })
            })?;
        }
    }
    cancellation.ensure_active()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::tests::{hook_child_command, is_hook_child_for};
    use crate::hooks::HookExecution;
    use std::fs;
    use tempfile::TempDir;

    fn config(dir: &TempDir) -> ResolvedSiteConfig {
        let config = crate::config::tests::load_test_config(dir.path(), "");
        fs::create_dir_all(&config.build.publish_dir).unwrap();
        config
    }

    /// The hook a test runs as its own command, with the test's name as the hook name.
    fn hook(test: &str) -> BeforeBuildHookConfig {
        hook_named(test, &format!("hooks::runner::tests::{test}"))
    }

    fn hook_named(name: &str, child: &str) -> BeforeBuildHookConfig {
        BeforeBuildHookConfig {
            name: name.into(),
            command: hook_child_command(child),
            ..BeforeBuildHookConfig::default()
        }
    }

    #[test]
    fn child_writes_declared_output() {
        if !is_hook_child_for("before-build") {
            return;
        }
        crate::build::tests::assert_hook_environment("before-build");
        fs::create_dir_all("generated").unwrap();
        fs::write("generated/site.css", "compiled").unwrap();
    }

    #[test]
    fn child_counts_successful_runs() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::create_dir_all("generated").unwrap();
        let count = fs::read_to_string("generated/count.txt")
            .ok()
            .map(|text| text.parse::<usize>().unwrap())
            .unwrap_or_default();
        fs::write("generated/count.txt", (count + 1).to_string()).unwrap();
    }

    #[test]
    fn child_writes_middle_chain_output() {
        if !is_hook_child_for("before-build") {
            return;
        }
        let count = fs::read_to_string("generated/count.txt").unwrap();
        fs::write("generated/middle.txt", count).unwrap();
        assert!(!Path::new("fail.txt").exists(), "middle command failed");
    }

    #[test]
    fn child_writes_final_chain_output() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::write(
            "generated/final.txt",
            fs::read_to_string("generated/middle.txt").unwrap(),
        )
        .unwrap();
    }

    /// One declaration that loading accepts and a retargeted symlink later aims at a reserved
    /// path: the check before each run refuses it, so the child command never writes.
    #[cfg(unix)]
    #[test]
    fn retargeted_outputs_stop_before_reserved_paths() {
        struct Case {
            /// The hook declaration loading accepts, as written in the configuration document.
            source: &'static str,
            /// The child command that would make the retargeted write observable.
            child: &'static str,
            /// Create the declared path as an ordinary entry, so loading accepts the declaration.
            prepare: fn(&Path),
            /// Retarget the declared path at a reserved path.
            retarget: fn(&Path),
            /// The boundary the refusal must name.
            refused_as: &'static str,
            /// Assert that what the child command would have written is unchanged.
            assert_preserved: fn(&Path, &str),
        }
        let cases = [
            Case {
                source: "[vendor]\npath = \"vendor\"\n\n[[build.hooks.before-build]]\nname = \"generator\"\ncommand = [\"generator\"]\ngenerates = [\"alias/marker.txt\"]\n",
                child: "child_writes_declared_output",
                prepare: |root| fs::create_dir(root.join("alias")).unwrap(),
                retarget: |root| {
                    fs::remove_dir(root.join("alias")).unwrap();
                    let workspace = root.join(".vendor-vendor");
                    fs::create_dir(&workspace).unwrap();
                    std::os::unix::fs::symlink(workspace, root.join("alias")).unwrap();
                },
                refused_as: "vendor workspace",
                assert_preserved: |root, _| assert!(!root.join("generated/site.css").exists()),
            },
            Case {
                source: "[[build.hooks.before-build]]\nname = \"generator\"\ncommand = [\"generator\"]\ngenerates = [\"alias.toml\"]\n",
                child: "child_rewrites_the_configuration",
                prepare: |root| fs::write(root.join("alias.toml"), "generated").unwrap(),
                retarget: |root| {
                    fs::remove_file(root.join("alias.toml")).unwrap();
                    std::os::unix::fs::symlink(root.join("tola.toml"), root.join("alias.toml"))
                        .unwrap();
                },
                refused_as: "site configuration",
                assert_preserved: |root, source| {
                    assert_eq!(fs::read_to_string(root.join("tola.toml")).unwrap(), source);
                },
            },
        ];

        for case in cases {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            (case.prepare)(root);
            let mut config = crate::config::tests::load_test_config(root, case.source);
            fs::create_dir_all(&config.build.publish_dir).unwrap();
            // The declaration stays as loaded; the child command only makes execution observable.
            config.build.hooks.before_build[0].command =
                hook_child_command(&format!("hooks::runner::tests::{}", case.child));
            (case.retarget)(root);

            let error = run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Production,
                HookExecution::Run,
                None,
            )
            .unwrap_err();

            (case.assert_preserved)(root, case.source);
            let diagnostics =
                crate::diagnostic::attached(&error.error).expect("the refused output is reported");
            assert_eq!(diagnostics[0].code, crate::codes::build::HOOKS);
            assert!(
                diagnostics[0].message.contains(case.refused_as),
                "{diagnostics:?}"
            );
        }
    }

    #[test]
    fn child_rewrites_the_configuration() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::write("alias.toml", "the hook rewrote the configuration").unwrap();
    }

    #[test]
    fn child_writes_partial_output_then_fails() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::create_dir_all("failed").unwrap();
        fs::write("failed/partial.txt", "partial").unwrap();
        panic!("generator failed after writing");
    }

    #[test]
    fn child_records_before_build_start() {
        if !is_hook_child_for("before-build") {
            return;
        }
        crate::build::tests::assert_hook_environment("before-build");
        fs::write("before-started.txt", "started").unwrap();
    }

    #[test]
    fn skipped_attempt_runs_no_hooks() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::write(root.join("site.typ"), "#document(\"index.html\")[Body]").unwrap();
        let mut config = config(&directory);
        config
            .build
            .hooks
            .before_build
            .push(hook("child_records_before_build_start"));
        let marker = config.get_root().join("before-started.txt");

        let mut request = crate::build::BuildRequest::new(BuildMode::Production);
        request.hook_execution = HookExecution::Skip;
        crate::build::BuildSession::new()
            .prepare(std::sync::Arc::new(config.clone()), request)
            .run()
            .unwrap();
        assert!(!marker.exists(), "a skipped attempt ran a hook command");

        let mut request = crate::build::BuildRequest::new(BuildMode::Production);
        request.hook_execution = HookExecution::Run;
        crate::build::BuildSession::new()
            .prepare(std::sync::Arc::new(config), request)
            .run()
            .unwrap();
        assert!(marker.exists(), "the hook did not run under Run");
    }

    #[test]
    fn participating_commands_run_each_build() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        let counter = hook("child_counts_successful_runs");
        let mut disabled = hook("child_counts_successful_runs");
        disabled.enable = false;
        let mut production_only = hook("child_writes_declared_output");
        production_only.dev = crate::config::section::build::DevParticipation::Skip;
        production_only.generates = vec!["generated/site.css".into()];
        config.build.hooks.before_build = vec![counter, disabled, production_only];

        for expected in ["1", "2"] {
            let execution = run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Development,
                HookExecution::Run,
                None,
            )
            .unwrap();
            assert!(execution.hooks_executed());
            assert_eq!(
                fs::read_to_string(config.get_root().join("generated/count.txt")).unwrap(),
                expected
            );
            assert!(!config.get_root().join("generated/site.css").exists());
        }

        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(config.get_root().join("generated/count.txt")).unwrap(),
            "3"
        );
        assert_eq!(
            fs::read_to_string(config.get_root().join("generated/site.css")).unwrap(),
            "compiled"
        );
    }

    #[test]
    fn failed_chain_restarts_from_the_start() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        let mut first = hook("child_counts_successful_runs");
        first.generates = vec!["generated/count.txt".into()];
        let mut middle = hook("child_writes_middle_chain_output");
        middle.generates = vec!["generated/middle.txt".into()];
        let mut last = hook("child_writes_final_chain_output");
        last.generates = vec!["generated/final.txt".into()];
        config.build.hooks.before_build = vec![first, middle, last];
        let root = config.get_root();

        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("generated/final.txt")).unwrap(),
            "1"
        );
        fs::write(root.join("fail.txt"), "fail").unwrap();
        let failed = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap_err();
        assert_eq!(
            fs::read_to_string(root.join("generated/count.txt")).unwrap(),
            "2"
        );
        assert_eq!(
            fs::read_to_string(root.join("generated/final.txt")).unwrap(),
            "1"
        );
        assert_eq!(
            failed
                .completed()
                .outputs()
                .iter()
                .map(HookOutputEvidence::logical_path)
                .collect::<Vec<_>>(),
            [root.join("generated/count.txt")]
        );
        let partial = failed.partial_outputs().unwrap();
        assert!(partial.is_current());
        assert!(partial.covers_path(&root.join("generated/middle.txt")));
        assert!(!partial.covers_path(&root.join("generated/final.txt")));

        fs::remove_file(root.join("fail.txt")).unwrap();
        let recovered = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap()
        .into_source_hooks();
        for path in [
            "generated/count.txt",
            "generated/middle.txt",
            "generated/final.txt",
        ] {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "3");
        }
        assert_eq!(recovered.outputs().len(), 3);
        assert!(
            recovered
                .outputs()
                .iter()
                .all(HookOutputEvidence::is_current)
        );
    }

    /// Blocks until Tola stops it, so a cancellation can observe what survives.
    #[test]
    fn child_waits_for_cancellation() {
        if !is_hook_child_for("before-build") {
            return;
        }
        crate::build::tests::record_hook_environment();
        fs::write("hook-ready.txt", "ready").unwrap();
        loop {
            std::thread::park();
        }
    }

    /// Every hook stage's child receives the mode the build runs in.
    #[test]
    fn hook_child_receives_build_mode() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.before_build.push(hook_named(
            "record",
            "build::tests::child_records_hook_environment",
        ));
        config.build.hooks.after_publish.push(
            crate::config::section::build::AfterPublishHookConfig {
                name: "record".into(),
                command: hook_child_command("build::tests::child_records_hook_environment"),
                dev: crate::config::section::build::DevParticipation::Run,
                ..Default::default()
            },
        );

        for (mode, expected) in [
            (BuildMode::Development, "dev"),
            (BuildMode::Production, "prod"),
        ] {
            run_before_build_hooks_with_cancellation(&config, mode, HookExecution::Run, None)
                .unwrap();
            assert_eq!(
                crate::build::tests::recorded_build_mode(&root),
                expected,
                "before-build {expected}"
            );
            run_after_publish_hooks(
                &config,
                mode,
                &config.build.publish_dir,
                &crate::cancellation::BuildCancellation::new(),
            )
            .unwrap();
            assert_eq!(
                crate::build::tests::recorded_build_mode(&root),
                expected,
                "after-publish {expected}"
            );
        }
    }

    /// A hook identity's cache directory, and what the command left in it, outlives both
    /// ways a run can end without finishing: a failed command and a cancelled one.
    #[test]
    fn cache_directory_survives_unfinished_run() {
        #[derive(Clone, Copy, Debug)]
        enum Ending {
            Failed,
            Cancelled,
        }
        for ending in [Ending::Failed, Ending::Cancelled] {
            let directory = TempDir::new().unwrap();
            let root = directory.path().to_path_buf();
            let mut config = config(&directory);
            config.build.hooks.before_build = vec![hook_named(
                "images",
                "build::tests::child_records_hook_environment",
            )];
            run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Production,
                HookExecution::Run,
                None,
            )
            .unwrap();
            let marker =
                crate::build::tests::recorded_hook_cache_directory(&root).join("marker.txt");
            fs::write(&marker, "kept").unwrap();

            match ending {
                Ending::Failed => {
                    config.build.hooks.before_build = vec![hook_named(
                        "images",
                        "hooks::runner::tests::child_writes_partial_output_then_fails",
                    )];
                    let error = run_before_build_hooks_with_cancellation(
                        &config,
                        BuildMode::Production,
                        HookExecution::Run,
                        None,
                    )
                    .unwrap_err();
                    assert!(
                        crate::diagnostic::attached(&error.error).is_some(),
                        "{ending:?}"
                    );
                }
                Ending::Cancelled => {
                    config.build.hooks.before_build = vec![hook_named(
                        "images",
                        "hooks::runner::tests::child_waits_for_cancellation",
                    )];
                    let canceller = crate::cancellation::BuildCanceller::new();
                    let cancellation = canceller.token();
                    let error = std::thread::scope(|scope| {
                        let worker = scope.spawn(|| {
                            run_before_build_hooks_with_cancellation(
                                &config,
                                BuildMode::Development,
                                HookExecution::Run,
                                Some(&cancellation),
                            )
                        });
                        let ready = root.join("hook-ready.txt");
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(30);
                        while !ready.exists() {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "the hook command never started"
                            );
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        canceller.cancel();
                        worker.join().unwrap()
                    })
                    .unwrap_err();
                    assert!(
                        error.error.is::<crate::cancellation::BuildCancelled>(),
                        "{ending:?}"
                    );
                }
            }

            assert_eq!(fs::read_to_string(&marker).unwrap(), "kept", "{ending:?}");
        }
    }
}
