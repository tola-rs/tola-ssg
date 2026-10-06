//! Runs trusted hook commands at their configured build stages.

use super::command::{HookCall, HookDirectories, HookInvocation};
use super::evidence::{
    FailedHookOutputs, HookOutputEvidence, SourceHookOutputs, declared_output_snapshots,
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
        let after = declared_output_snapshots(hook, config, cancellation)?;
        for snapshot in &after {
            if snapshot.entries.is_empty() {
                let declared = crate::filesystem::display_path(
                    snapshot.source_identity.logical_path(),
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
            .map(|snapshot| HookOutputEvidence::from_successful_snapshot(snapshot, cancellation))
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
    fn writes_nothing() {
        if is_hook_child_for("before-build") {
            crate::build::tests::assert_hook_environment("before-build");
        }
    }

    #[test]
    fn writes_declared_output() {
        if !is_hook_child_for("before-build") {
            return;
        }
        crate::build::tests::assert_hook_environment("before-build");
        fs::create_dir_all("generated").unwrap();
        fs::write("generated/site.css", "compiled").unwrap();
    }

    #[test]
    fn counts_successful_runs() {
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
    fn writes_middle_chain_output() {
        if !is_hook_child_for("before-build") {
            return;
        }
        let count = fs::read_to_string("generated/count.txt").unwrap();
        fs::write("generated/middle.txt", count).unwrap();
        assert!(!Path::new("fail.txt").exists(), "middle command failed");
    }

    #[test]
    fn writes_final_chain_output() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::write(
            "generated/final.txt",
            fs::read_to_string("generated/middle.txt").unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn executed_hook_reports_its_output() {
        let dir = TempDir::new().unwrap();
        let mut config = config(&dir);
        let root = config.get_root().to_path_buf();
        fs::create_dir_all(root.join("generated")).unwrap();
        fs::write(root.join("generated/site.css"), "before").unwrap();
        let mut generator = hook("writes_declared_output");
        generator.generates = vec!["generated/site.css".into()];
        config.build.hooks.before_build.push(generator);

        let execution = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap();
        assert!(execution.hooks_executed());
        assert_eq!(
            fs::read_to_string(root.join("generated/site.css")).unwrap(),
            "compiled"
        );
        let evidence = execution.into_source_hooks();
        assert_eq!(evidence.outputs().len(), 1);
        assert_eq!(
            evidence.outputs()[0].logical_path(),
            root.join("generated/site.css")
        );
        assert!(evidence.outputs()[0].is_current());
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
                child: "writes_declared_output",
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
                child: "rewrites_the_configuration",
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
    fn rewrites_the_configuration() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::write("alias.toml", "the hook rewrote the configuration").unwrap();
    }

    #[test]
    fn evidence_keeps_snapshotted_kind() {
        let directory = TempDir::new().unwrap();
        let config = config(&directory);
        let output = config.get_root().join("generated");
        fs::create_dir(&output).unwrap();
        let mut hook = hook("writes_nothing");
        hook.generates = vec!["generated".into()];
        let snapshot = declared_output_snapshots(&hook, &config, None)
            .unwrap()
            .pop()
            .unwrap();

        fs::remove_dir(&output).unwrap();
        fs::write(&output, "replaced with a file").unwrap();
        let evidence = HookOutputEvidence::from_successful_snapshot(snapshot, None).unwrap();

        assert!(evidence.covers_path(&output.join("nested.css")));
    }

    #[test]
    fn writes_partial_output_then_fails() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::create_dir_all("failed").unwrap();
        fs::write("failed/partial.txt", "partial").unwrap();
        panic!("generator failed after writing");
    }

    #[test]
    fn failed_chain_separates_partial_files() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        let mut first = hook("writes_declared_output");
        first.generates = vec!["generated/site.css".into()];
        let mut second = hook("writes_partial_output_then_fails");
        second.generates = vec!["failed/partial.txt".into(), "failed/missing.txt".into()];
        config.build.hooks.before_build = vec![first, second];
        let error = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap_err();
        let diagnostics = crate::diagnostic::attached(&error.error)
            .expect("a failed hook command has its diagnostic");
        assert_eq!(diagnostics[0].code, "hook.command");
        assert!(
            diagnostics[0]
                .notes
                .iter()
                .any(|note| note.contains("generator failed after writing")),
            "{:?}",
            diagnostics[0].notes
        );
        assert_eq!(error.completed.outputs().len(), 1);
        assert_eq!(
            error.completed.outputs()[0].logical_path(),
            config.get_root().join("generated/site.css")
        );
        let outputs = error.partial.as_ref().unwrap();
        assert!(outputs.is_current());
        for path in ["failed/partial.txt", "failed/missing.txt"] {
            assert!(outputs.covers_path(&config.get_root().join(path)));
        }
        assert!(!outputs.covers_path(&config.get_root().join("content/page.typ")));

        fs::write(config.get_root().join("failed/missing.txt"), "now present").unwrap();
        assert!(!outputs.is_current());
    }

    #[cfg(unix)]
    #[test]
    fn failed_outputs_track_symlink_target() {
        use std::os::unix::fs::symlink;
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("first.txt"), "same").unwrap();
        fs::write(directory.path().join("second.txt"), "same").unwrap();
        let output = directory.path().join("generated.txt");
        symlink("first.txt", &output).unwrap();
        let outputs = FailedHookOutputs::capture(&BTreeSet::from([output.clone()]), None)
            .unwrap()
            .unwrap();
        assert!(outputs.is_current());

        fs::remove_file(&output).unwrap();
        symlink("second.txt", &output).unwrap();
        assert!(!outputs.is_current());
    }

    /// A path check answers for the paths it was asked about, and only those.
    #[test]
    fn selected_paths_keep_their_own_freshness() {
        let directory = TempDir::new().unwrap();
        let first = directory.path().join("first.txt");
        let second = directory.path().join("second.txt");
        fs::write(&first, "observed").unwrap();
        fs::write(&second, "observed").unwrap();
        let outputs =
            FailedHookOutputs::capture(&BTreeSet::from([first.clone(), second.clone()]), None)
                .unwrap()
                .unwrap();
        fs::write(&second, "changed independently").unwrap();
        let cancellation = crate::cancellation::BuildCancellation::new();

        let current = outputs
            .current_for_paths(std::slice::from_ref(&first), &cancellation)
            .unwrap()
            .unwrap();

        assert!(current.covers_path(&first));
        assert!(!current.covers_path(&second));
        assert!(
            outputs
                .current_for_paths(std::slice::from_ref(&second), &cancellation)
                .unwrap()
                .is_none()
        );
    }

    /// A cancelled token stops a path check instead of answering it.
    #[test]
    fn cancelled_path_check_stops() {
        let directory = TempDir::new().unwrap();
        let first = directory.path().join("first.txt");
        fs::write(&first, "observed").unwrap();
        let outputs = FailedHookOutputs::capture(&BTreeSet::from([first.clone()]), None)
            .unwrap()
            .unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();

        assert!(matches!(
            outputs.current_for_paths(&[first], &canceller.token()),
            Err(crate::cancellation::BuildCancelled),
        ));
    }

    #[test]
    fn records_after_hook_start() {
        if !is_hook_child_for("after-publish") {
            return;
        }
        crate::build::tests::assert_hook_environment("after-publish");
        fs::write("after-started.txt", "started").unwrap();
    }

    #[test]
    fn records_before_build_start() {
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
            .push(hook("records_before_build_start"));
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
    fn cancelled_after_hook_never_starts() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        config.build.hooks.after_publish.push(
            crate::config::section::build::AfterPublishHookConfig {
                name: "records after-publish start".into(),
                command: hook_child_command("hooks::runner::tests::records_after_hook_start"),
                ..Default::default()
            },
        );
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        let error = run_after_publish_hooks(
            &config,
            BuildMode::Production,
            &config.build.publish_dir,
            &cancellation,
        )
        .unwrap_err();

        assert!(error.is::<crate::cancellation::BuildCancelled>());
        assert!(!config.get_root().join("after-started.txt").exists());
    }

    #[test]
    fn participating_commands_run_each_build() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        let counter = hook("counts_successful_runs");
        let mut disabled = hook("counts_successful_runs");
        disabled.enable = false;
        let mut production_only = hook("writes_declared_output");
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
        let mut first = hook("counts_successful_runs");
        first.generates = vec!["generated/count.txt".into()];
        let mut middle = hook("writes_middle_chain_output");
        middle.generates = vec!["generated/middle.txt".into()];
        let mut last = hook("writes_final_chain_output");
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

    #[test]
    fn cancelled_before_build_starts_nothing() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        config
            .build
            .hooks
            .before_build
            .push(hook("counts_successful_runs"));
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        let error = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            Some(&cancellation),
        )
        .unwrap_err();

        assert!(error.error.is::<crate::cancellation::BuildCancelled>());
        assert!(!config.get_root().join("generated/count.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn declared_output_stays_out_of_internal() {
        let dir = TempDir::new().unwrap();
        let config = config(&dir);
        fs::create_dir_all(dir.path().join(crate::filesystem::INTERNAL_DIR)).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join(crate::filesystem::INTERNAL_DIR),
            dir.path().join("generated"),
        )
        .unwrap();
        let mut hook = hook("writes_nothing");
        hook.generates.push("generated/cache.bin".into());

        let error = run_before_build_hook(
            &hook,
            &config,
            BuildMode::Production,
            None,
            &mut BTreeSet::new(),
        )
        .unwrap_err();
        let diagnostics =
            crate::diagnostic::attached(&error).expect("the refused output is reported");
        assert_eq!(diagnostics[0].code, crate::codes::build::HOOKS);
        assert!(diagnostics[0].message.contains("generated/cache.bin"));
    }

    #[test]
    fn before_build_requires_declared_outputs() {
        let dir = TempDir::new().unwrap();
        let config = config(&dir);
        let mut hook = hook("writes_nothing");
        hook.generates.push("generated/missing.css".into());

        let error = run_before_build_hook(
            &hook,
            &config,
            BuildMode::Production,
            None,
            &mut BTreeSet::new(),
        )
        .unwrap_err();

        let diagnostics =
            crate::diagnostic::attached(&error).expect("the refused output is reported");
        assert_eq!(diagnostics[0].code, crate::codes::build::HOOKS);
        assert!(diagnostics[0].message.contains("build.hooks.before-build"));
        assert!(diagnostics[0].message.contains("generated/missing.css"));
    }

    /// Blocks until Tola stops it, so a cancellation can observe what survives.
    #[test]
    fn waits_for_cancellation() {
        if !is_hook_child_for("before-build") {
            return;
        }
        crate::build::tests::record_hook_environment();
        fs::write("hook-ready.txt", "ready").unwrap();
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn before_build_child_receives_build_mode() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.before_build.push(hook_named(
            "record",
            "build::tests::records_hook_environment",
        ));

        for (mode, expected) in [
            (BuildMode::Development, "dev"),
            (BuildMode::Production, "prod"),
        ] {
            run_before_build_hooks_with_cancellation(&config, mode, HookExecution::Run, None)
                .unwrap();
            assert_eq!(crate::build::tests::recorded_build_mode(&root), expected);
        }
    }

    #[test]
    fn after_publish_child_receives_build_mode() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.after_publish.push(
            crate::config::section::build::AfterPublishHookConfig {
                name: "record".into(),
                command: hook_child_command("build::tests::records_hook_environment"),
                dev: crate::config::section::build::DevParticipation::Run,
                ..Default::default()
            },
        );

        for (mode, expected) in [
            (BuildMode::Development, "dev"),
            (BuildMode::Production, "prod"),
        ] {
            run_after_publish_hooks(
                &config,
                mode,
                &config.build.publish_dir,
                &crate::cancellation::BuildCancellation::new(),
            )
            .unwrap();
            assert_eq!(crate::build::tests::recorded_build_mode(&root), expected);
        }
    }

    /// One hook identity owns one cache directory across builds: the build mode, the command
    /// arguments, and the configuration document naming it do not change that directory.
    #[test]
    fn repeated_hooks_share_one_cache_directory() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::records_hook_environment",
        )];

        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let cache = crate::build::tests::recorded_hook_cache_directory(&root);
        assert!(cache.is_dir());
        assert!(!crate::build::tests::recorded_temporary_directory(&root).exists());

        let mut changed_command = hook_named("images", "build::tests::records_hook_environment");
        changed_command.command.push("--test-threads=1".into());
        config.build.hooks.before_build = vec![
            hook_named("other", "build::tests::records_hook_environment"),
            changed_command,
        ];
        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap();

        let other_document = root.join("other.toml");
        fs::write(
            &other_document,
            "[[build.hooks.before-build]]\nname = \"images\"\ncommand = [\"true\"]\n",
        )
        .unwrap();
        let mut reloaded = crate::config::loading::load_site_config(
            Some(&other_document),
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        reloaded.build.hooks.before_build[0].command =
            hook_child_command("build::tests::records_hook_environment");
        run_before_build_hooks_with_cancellation(
            &reloaded,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap();

        assert_eq!(
            crate::build::tests::recorded_hook_cache_directory(&root),
            cache
        );
        assert_eq!(
            fs::read_to_string(cache.join("invocations.txt"))
                .unwrap()
                .lines()
                .count(),
            3
        );
    }

    /// A configured name is one identity, whatever characters it has: no name reaches
    /// outside the site root, and no two names share a cache directory.
    #[test]
    fn distinct_hook_names_keep_distinct_caches() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut caches = std::collections::BTreeSet::new();
        for name in [
            "images",
            "Images",
            "图片",
            "a/b",
            "..",
            "a.b",
            "images/nested",
            "../../../../escape",
        ] {
            let mut config = config(&directory);
            config.build.hooks.before_build =
                vec![hook_named(name, "build::tests::records_hook_environment")];
            run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Development,
                HookExecution::Run,
                None,
            )
            .unwrap();
            let cache = crate::build::tests::recorded_hook_cache_directory(&root);
            assert!(
                cache.canonicalize().unwrap().starts_with(&root),
                "`{name}` escaped the site root"
            );
            assert!(caches.insert(cache), "`{name}` reused another name's cache");
        }

        // Renaming the hook gives it a cache of its own and leaves the old one in place.
        let mut config = config(&directory);
        config.build.hooks.before_build = vec![hook_named(
            "renamed",
            "build::tests::records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let renamed = crate::build::tests::recorded_hook_cache_directory(&root);
        assert!(!caches.contains(&renamed));
        assert!(caches.iter().all(|cache| cache.is_dir()));
    }

    /// The cache directory separates stages and sites, not names alone.
    #[test]
    fn hook_cache_directory_isolates_stage_and_site() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut site = config(&directory);
        site.build.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &site,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let before_build = crate::build::tests::recorded_hook_cache_directory(&root);

        site.build.hooks.after_publish.push(
            crate::config::section::build::AfterPublishHookConfig {
                name: "images".into(),
                command: hook_child_command("build::tests::records_hook_environment"),
                ..Default::default()
            },
        );
        run_after_publish_hooks(
            &site,
            BuildMode::Production,
            &site.build.publish_dir,
            &crate::cancellation::BuildCancellation::new(),
        )
        .unwrap();
        let after_publish = crate::build::tests::recorded_hook_cache_directory(&root);
        assert_ne!(after_publish, before_build);

        let elsewhere = TempDir::new().unwrap();
        let mut other = config(&elsewhere);
        other.build.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &other,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let other_site = crate::build::tests::recorded_hook_cache_directory(elsewhere.path());
        assert_ne!(other_site, before_build);
    }

    #[test]
    fn blocked_cache_directory_stops_the_hook() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let cache = crate::build::tests::recorded_hook_cache_directory(&root);
        fs::remove_dir_all(&cache).unwrap();
        fs::write(&cache, "not a directory").unwrap();

        config.build.hooks.before_build = vec![hook_named(
            "images",
            "hooks::runner::tests::records_before_build_start",
        )];
        let error = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap_err();

        assert!(!root.join("before-started.txt").exists());
        let diagnostics =
            crate::diagnostic::attached(&error.error).expect("the failure is reported");
        assert_eq!(diagnostics[0].code, crate::codes::hook::COMMAND);
        assert!(diagnostics[0].message.contains("images"));
        assert!(diagnostics[0].message.contains("build.hooks.before-build"));
        assert!(!diagnostics[0].message.contains(root.to_str().unwrap()));
    }

    #[test]
    fn failed_hook_keeps_its_cache_directory() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let marker = crate::build::tests::recorded_hook_cache_directory(&root).join("marker.txt");
        fs::write(&marker, "kept").unwrap();

        config.build.hooks.before_build = vec![hook_named(
            "images",
            "hooks::runner::tests::writes_partial_output_then_fails",
        )];
        let error = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Production,
            HookExecution::Run,
            None,
        )
        .unwrap_err();

        assert!(crate::diagnostic::attached(&error.error).is_some());
        assert_eq!(fs::read_to_string(&marker).unwrap(), "kept");
    }

    #[test]
    fn cancelled_hook_keeps_its_cache_directory() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().to_path_buf();
        let mut config = config(&directory);
        config.build.hooks.before_build = vec![hook_named(
            "images",
            "build::tests::records_hook_environment",
        )];
        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            HookExecution::Run,
            None,
        )
        .unwrap();
        let marker = crate::build::tests::recorded_hook_cache_directory(&root).join("marker.txt");
        fs::write(&marker, "kept").unwrap();

        config.build.hooks.before_build = vec![hook_named(
            "images",
            "hooks::runner::tests::waits_for_cancellation",
        )];
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let outcome = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                run_before_build_hooks_with_cancellation(
                    &config,
                    BuildMode::Development,
                    HookExecution::Run,
                    Some(&cancellation),
                )
            });
            let ready = root.join("hook-ready.txt");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !ready.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the hook command never started"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            canceller.cancel();
            worker.join().unwrap()
        });

        let error = outcome.unwrap_err();
        assert!(error.error.is::<crate::cancellation::BuildCancelled>());
        assert_eq!(fs::read_to_string(&marker).unwrap(), "kept");
        assert!(!crate::build::tests::recorded_temporary_directory(&root).exists());
    }
}
