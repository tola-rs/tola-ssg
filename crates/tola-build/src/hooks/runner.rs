//! Runs trusted hook commands at their configured build stages.

use super::command::{HookCall, HookDirectories, ObserveFailure, run_action};
use super::evidence::{
    FailedHookOutputs, HookOutputEvidence, SourceHookOutputs, declared_output_files,
    validate_declared_output_boundaries,
};
use super::report::{
    BatchFailure, failed_to_start, genuinely_failed, hook_command_error, report_reused,
    run_hook_entry,
};
use super::reuse::{BeforeBuildInputs, HookId, PreparedHook, PreparedHooks};
use super::{HookExecutor, hook_batches, hook_participates};

use crate::cancellation::{BuildCancellation, OptionalCancellation};
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::BeforeBuildHookConfig;
use crate::config::section::build::hooks::HookStage;
use crate::mode::BuildMode;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub(crate) struct SourceHookFailure {
    #[source]
    error: anyhow::Error,
    completed: SourceHookOutputs,
    partial: Option<FailedHookOutputs>,
}

impl SourceHookFailure {
    fn new(
        error: anyhow::Error,
        completed: SourceHookOutputs,
        paths: &BTreeSet<PathBuf>,
        cancellation: Option<&BuildCancellation>,
    ) -> Self {
        let partial = if crate::cancellation::is_cancelled(&error) {
            None
        } else {
            FailedHookOutputs::capture(paths, cancellation)
                .ok()
                .flatten()
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

#[allow(clippy::too_many_arguments)]
fn run_before_build_hook(
    hook: &BeforeBuildHookConfig,
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    cancellation: Option<&BuildCancellation>,
    executed_outputs: &mut BTreeSet<PathBuf>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
    observe_failure: &ObserveFailure,
) -> Result<Vec<HookOutputEvidence>> {
    cancellation.ensure_active_if_present()?;
    let identity = super::hook_identity(HookStage::BeforeBuild, &hook.name);
    let before = validate_declared_output_boundaries(hook, config, &config.build.publish_dir)?;
    executed_outputs.extend(
        before
            .iter()
            .map(|identity| identity.logical_path().to_path_buf()),
    );
    run_action(
        config,
        hook.action(),
        HookCall {
            site: super::command::HookSitePaths::new(config, scope),
            name: &hook.name,
            origin: hook.origin.as_ref(),
            mode,
            directories: HookDirectories::BeforeBuild,
        },
        &hook.rerun_on,
        cancellation,
        diagnostics,
        observe_failure,
    )
    .map_err(|error| hook_command_error(HookStage::BeforeBuild, &identity, error))?;
    let after = declared_output_files(hook, config, cancellation)?;
    for files in &after {
        if files.contents.is_none() {
            let declared = tola_diagnostic::display_path(
                files.source_identity.logical_path(),
                config.get_root(),
            );
            return Err(super::hook_contract_error(
                format!("{identity} declares `{declared}`, but the hook wrote nothing"),
                "write it, or remove it from `generates`",
            ));
        }
    }
    after
        .into_iter()
        .map(|files| HookOutputEvidence::from_successful_output(files, cancellation))
        .collect()
}

struct Preparation {
    outputs: Vec<HookOutputEvidence>,
    cache: Option<Arc<PreparedHook>>,
    executed: bool,
}

/// Site and cache shared by preparations in this build. Each ordered batch reads
/// its inputs after the preceding batch has finished writing its generated files.
struct BeforeBuildHooks<'a> {
    config: &'a ResolvedSiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    previous: &'a PreparedHooks,
}

impl BeforeBuildHooks<'_> {
    fn prepare(
        &self,
        hook: &BeforeBuildHookConfig,
        id: &HookId,
        stop: &BuildCancellation,
        started_outputs: &mut BTreeSet<PathBuf>,
        reported: &mut Vec<tola_diagnostic::Diagnostic>,
        observe_failure: &ObserveFailure,
    ) -> Result<Preparation> {
        let inputs = if self.mode == BuildMode::Development
            && hook.dev.reuses_outputs()
            && !hook.generates.is_empty()
        {
            BeforeBuildInputs::observe(self.config, hook, self.scope, stop)?
        } else {
            None
        };
        if let Some(inputs) = &inputs
            && let Some(saved) = self.previous.matching(id, inputs, stop)?
        {
            validate_declared_output_boundaries(hook, self.config, &self.config.build.publish_dir)?;
            report_reused(HookStage::BeforeBuild, &hook.name);
            reported.extend_from_slice(&saved.diagnostics);
            return Ok(Preparation {
                outputs: saved.outputs.clone(),
                cache: Some(saved),
                executed: false,
            });
        }
        let outputs = run_hook_entry(HookStage::BeforeBuild, &hook.name, stop, || {
            run_before_build_hook(
                hook,
                self.config,
                self.mode,
                self.scope,
                Some(stop),
                started_outputs,
                reported,
                observe_failure,
            )
        })?;
        // A generator can run while its inputs change. Do not retain its
        // outputs under an observation that no longer describes those inputs.
        let cache = if let Some(before) = inputs
            && BeforeBuildInputs::observe(self.config, hook, self.scope, stop)?.as_ref()
                == Some(&before)
        {
            Some(Arc::new(PreparedHook {
                inputs: before,
                outputs: outputs.clone(),
                diagnostics: reported.clone(),
            }))
        } else {
            None
        };
        Ok(Preparation {
            outputs,
            cache,
            executed: true,
        })
    }
}

pub(crate) fn run_before_build_hooks_with_cancellation(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    cache: &mut PreparedHooks,
    cancellation: Option<&BuildCancellation>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
    executor: &HookExecutor,
) -> Result<BeforeBuildExecution, SourceHookFailure> {
    let previous = std::mem::take(cache);
    let commands = BeforeBuildHooks {
        config,
        mode,
        scope,
        previous: &previous,
    };
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
    let stage = HookStage::BeforeBuild;
    let hooks = &config.hooks.before_build;
    for mut batch in hook_batches(
        hooks,
        true,
        |hook| hook.origin.as_ref().map(|origin| origin.name.as_str()),
        |hook| hook.order,
    ) {
        batch.retain(|&index| hook_participates(hooks[index].enable, hooks[index].dev, mode));
        let results = executor.run_batch(
            batch.len(),
            cancellation,
            |member, stop, observe_failure| {
                let hook = &hooks[batch[member]];
                let mut started_outputs = BTreeSet::new();
                let mut reported = Vec::new();
                let id = HookId::new(
                    hook.origin.as_ref(),
                    &hook.name,
                    hook.declared_name.as_deref(),
                );
                let outputs = commands.prepare(
                    hook,
                    &id,
                    stop,
                    &mut started_outputs,
                    &mut reported,
                    observe_failure,
                );
                if genuinely_failed(&outputs) {
                    observe_failure();
                }
                (outputs, id, started_outputs, reported)
            },
            |(outputs, _, _, _)| genuinely_failed(outputs),
        );
        let mut failure = BatchFailure::default();
        let mut partial_paths = BTreeSet::new();
        for (member, result) in results.into_iter().enumerate() {
            match result {
                Some(Ok((outputs, id, started_outputs, reported))) => {
                    diagnostics.extend(reported);
                    match outputs {
                        Ok(prepared) => {
                            if let Some(entry) = prepared.cache {
                                cache.entries.insert(id, entry);
                            }
                            completed.outputs.extend(prepared.outputs);
                            hooks_executed |= prepared.executed;
                        }
                        Err(error) => {
                            partial_paths.extend(started_outputs);
                            failure.record(error, diagnostics);
                        }
                    }
                }
                Some(Err(error)) => {
                    let name = &hooks[batch[member]].name;
                    failure.record(failed_to_start(stage, name, error), diagnostics);
                }
                None => {}
            }
        }
        if let Some(error) = failure.error() {
            return Err(SourceHookFailure::new(
                error,
                completed,
                &partial_paths,
                cancellation,
            ));
        }
        if let Err(error) = cancellation.ensure_active_if_present() {
            return Err(SourceHookFailure::new(
                error.into(),
                completed,
                &partial_paths,
                None,
            ));
        }
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
        .hooks
        .after_publish
        .iter()
        .any(|hook| super::hook_participates(hook.enable, hook.dev, mode))
}

pub(super) fn after_publish_batches(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
) -> impl Iterator<Item = Vec<usize>> {
    let hooks = &config.hooks.after_publish;
    hook_batches(
        hooks,
        false,
        |hook| hook.origin.as_ref().map(|origin| origin.name.as_str()),
        |hook| hook.order,
    )
    .into_iter()
    .filter_map(move |mut batch| {
        batch.retain(|&index| hook_participates(hooks[index].enable, hooks[index].dev, mode));
        (!batch.is_empty()).then_some(batch)
    })
}

/// Consume one committed output view using every participating hook.
pub fn run_after_publish_hooks(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    output_root: &Path,
    cancellation: &BuildCancellation,
    executor: &HookExecutor,
) -> Result<Vec<tola_diagnostic::Diagnostic>> {
    run_after_publish_hooks_observed(
        config,
        mode,
        scope,
        output_root,
        cancellation,
        executor,
        |_, _| Ok(()),
    )
}

/// Run after-publish hooks and return the diagnostics from successful reports.
pub fn run_after_publish_hooks_observed(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    output_root: &Path,
    cancellation: &BuildCancellation,
    executor: &HookExecutor,
    mut completed: impl FnMut(&str, &Result<Vec<tola_diagnostic::Diagnostic>>) -> Result<()>,
) -> Result<Vec<tola_diagnostic::Diagnostic>> {
    let mut diagnostics = Vec::new();
    let stage = HookStage::AfterPublish;
    let hooks = &config.hooks.after_publish;
    for batch in after_publish_batches(config, mode) {
        cancellation.ensure_active()?;
        let results = executor.run_batch(
            batch.len(),
            Some(cancellation),
            |member, stop, observe_failure| {
                let hook = &hooks[batch[member]];
                let identity = super::hook_identity(stage, &hook.name);
                run_hook_entry(stage, &hook.name, stop, || {
                    let mut reported = Vec::new();
                    let result = run_action(
                        config,
                        hook.action(),
                        HookCall {
                            site: super::command::HookSitePaths::new(config, scope),
                            name: &hook.name,
                            origin: hook.origin.as_ref(),
                            mode,
                            directories: HookDirectories::AfterPublish { input: output_root },
                        },
                        &hook.rerun_on,
                        Some(stop),
                        &mut reported,
                        observe_failure,
                    );
                    if genuinely_failed(&result) {
                        observe_failure();
                    }
                    result.map_err(|error| hook_command_error(stage, &identity, error))?;
                    Ok(reported)
                })
            },
            genuinely_failed,
        );
        let mut failure = BatchFailure::default();
        let mut callback_error = None;
        for (member, result) in results.into_iter().enumerate() {
            let hook = &hooks[batch[member]];
            let outcome = match result {
                Some(Ok(outcome)) => outcome,
                Some(Err(error)) => Err(failed_to_start(stage, &hook.name, error)),
                None => continue,
            };
            if let Err(error) = completed(&hook.name, &outcome) {
                callback_error.get_or_insert(error);
            }
            match outcome {
                Ok(reported) => diagnostics.extend(reported),
                Err(error) => failure.record(error, &mut diagnostics),
            }
        }
        if let Some(error) = callback_error {
            return Err(error);
        }
        if let Some(error) = failure.error() {
            if diagnostics.is_empty() {
                return Err(error);
            }
            if !crate::cancellation::is_cancelled(&error) {
                diagnostics
                    .extend_from_slice(tola_diagnostic::attached(&error).unwrap_or_default());
            }
            return Err(tola_diagnostic::DiagnosticError::attach(error, diagnostics).into());
        }
    }
    cancellation.ensure_active()?;
    Ok(diagnostics)
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
            argv: Some(hook_child_command(child)),
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
    fn prepare_reuse_checks_inputs_and_the_files_left_on_disk() {
        use crate::config::section::build::DevParticipation;
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        let mut prepare = hook("child_counts_successful_runs");
        prepare.dev = DevParticipation::Reuse;
        prepare.generates = vec!["generated/count.txt".into()];
        prepare
            .rerun_on
            .include
            .push(tola_module::hooks::RerunPath::Relative(
                "inputs/*.txt".into(),
            ));
        config.hooks.before_build.push(prepare);
        let mut cache = PreparedHooks::default();
        let executor = HookExecutor::new();
        let mut run = |mode| {
            run_before_build_hooks_with_cancellation(
                &config,
                mode,
                tola_input::InputScope::Online,
                &mut cache,
                None,
                &mut Vec::new(),
                &executor,
            )
            .unwrap()
            .hooks_executed()
        };
        let dev = BuildMode::Development;
        let root = config.get_root();
        let generated = root.join("generated/count.txt");
        assert!(run(dev));
        assert!(!run(dev));
        fs::create_dir(root.join("inputs")).unwrap();
        let input = root.join("inputs/new.txt");
        fs::write(&input, "new").unwrap();
        assert!(run(dev));
        fs::write(&input, "edited").unwrap();
        assert!(run(dev));
        fs::remove_file(input).unwrap();
        assert!(run(dev));
        fs::write(&generated, "20").unwrap();
        assert!(run(dev));
        assert_eq!(fs::read_to_string(&generated).unwrap(), "21");
        fs::remove_file(&generated).unwrap();
        assert!(run(dev));
        assert_eq!(fs::read_to_string(&generated).unwrap(), "1");
        assert!(!run(dev));
        assert!(run(BuildMode::Production));
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
    fn retargeted_outputs_stop() {
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
                source: "[vendor]\npath = \"vendor\"\n\n[[hooks.before-build]]\nname = \"generator\"\nargv = [\"generator\"]\ngenerates = [\"alias/marker.txt\"]\n",
                child: "child_writes_declared_output",
                prepare: |root| fs::create_dir(root.join("alias")).unwrap(),
                retarget: |root| {
                    fs::remove_dir(root.join("alias")).unwrap();
                    let workspace =
                        crate::filesystem::swap_workspace(&root.join("vendor")).unwrap();
                    fs::create_dir(&workspace).unwrap();
                    std::os::unix::fs::symlink(workspace, root.join("alias")).unwrap();
                },
                refused_as: "vendor workspace",
                assert_preserved: |root, _| assert!(!root.join("generated/site.css").exists()),
            },
            Case {
                source: "[[hooks.before-build]]\nname = \"generator\"\nargv = [\"generator\"]\ngenerates = [\"alias.toml\"]\n",
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
            config.hooks.before_build[0].argv = Some(hook_child_command(&format!(
                "hooks::runner::tests::{}",
                case.child
            )));
            (case.retarget)(root);

            let error = run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Production,
                tola_input::InputScope::Online,
                &mut PreparedHooks::default(),
                None,
                &mut Vec::new(),
                &HookExecutor::new(),
            )
            .unwrap_err();

            (case.assert_preserved)(root, case.source);
            let diagnostics =
                tola_diagnostic::attached(&error.error).expect("the refused output is reported");
            assert_eq!(diagnostics[0].code, tola_diagnostic::codes::build::HOOKS);
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
    fn child_writes_partial_then_fails() {
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
        config.hooks.before_build = vec![counter, disabled, production_only];

        for expected in ["1", "2"] {
            let execution = run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Development,
                tola_input::InputScope::Online,
                &mut PreparedHooks::default(),
                None,
                &mut Vec::new(),
                &HookExecutor::new(),
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
            tola_input::InputScope::Online,
            &mut PreparedHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
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
    fn failed_chain_restarts() {
        let directory = TempDir::new().unwrap();
        let mut config = config(&directory);
        let mut first = hook("child_counts_successful_runs");
        first.generates = vec!["generated/count.txt".into()];
        let mut middle = hook("child_writes_middle_chain_output");
        middle.order = 10;
        middle.generates = vec!["generated/middle.txt".into()];
        let mut last = hook("child_writes_final_chain_output");
        last.order = 20;
        last.generates = vec!["generated/final.txt".into()];
        config.hooks.before_build = vec![first, middle, last];
        let root = config.get_root();

        run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            tola_input::InputScope::Online,
            &mut PreparedHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
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
            tola_input::InputScope::Online,
            &mut PreparedHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
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
            tola_input::InputScope::Online,
            &mut PreparedHooks::default(),
            None,
            &mut Vec::new(),
            &HookExecutor::new(),
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
    fn hook_runner_panic_keeps_batch_output_evidence() {
        use crate::config::section::build::HookJobs;
        use crate::hooks::HookRunner;
        use crate::hooks::report::{CommandFailure, InterruptionStep};
        use std::num::NonZeroUsize;
        use std::sync::{Mutex, mpsc};
        use std::time::Duration;
        use tola_diagnostic::{Diagnostic, Severity, codes};

        let directory = TempDir::new().unwrap();
        let (completed_peer, wait_for_peer) = mpsc::channel();
        let wait_for_peer = Mutex::new(wait_for_peer);
        let mut config = config(&directory).with_hook_runner(HookRunner::new(move |hook| {
            let root = hook.config.get_root();
            match hook.context.hook.as_ref().unwrap().name.as_str() {
                "partial" => {
                    fs::write(root.join("generated/partial.txt"), "partial")?;
                    wait_for_peer
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))?;
                    panic!("private hook runner panic payload");
                }
                "complete" => {
                    fs::write(root.join("generated/complete.txt"), "complete")?;
                    Ok(vec![Diagnostic::new(
                        codes::hook::COMMAND,
                        Severity::Warning,
                        "The completed hook supplied this diagnostic",
                    )])
                }
                "witness" => {
                    // Two permits admit this member only after the complete hook
                    // has collected its output evidence and relinquished a permit.
                    completed_peer.send(())?;
                    Ok(Vec::new())
                }
                _ => unreachable!(),
            }
        }));
        let root = config.get_root().to_owned();
        fs::write(root.join("hook.nu"), "").unwrap();
        fs::create_dir(root.join("generated")).unwrap();
        config.hooks.before_build = [
            ("partial", vec!["generated/partial.txt".into()]),
            ("complete", vec!["generated/complete.txt".into()]),
            ("witness", Vec::new()),
        ]
        .into_iter()
        .map(|(name, generates)| BeforeBuildHookConfig {
            name: name.into(),
            nu: Some("hook.nu".into()),
            generates,
            ..Default::default()
        })
        .collect();

        let executor = HookExecutor::new();
        let cancellation = BuildCancellation::new();
        executor
            .configure(
                executor.begin_attempt(),
                true,
                HookJobs::Limit(NonZeroUsize::new(2).unwrap()),
                &cancellation,
            )
            .unwrap();
        let mut diagnostics = Vec::new();
        let failure = run_before_build_hooks_with_cancellation(
            &config,
            BuildMode::Development,
            tola_input::InputScope::Online,
            &mut PreparedHooks::default(),
            Some(&cancellation),
            &mut diagnostics,
            &executor,
        )
        .unwrap_err();

        assert!(failure.error.chain().any(|cause| matches!(
            cause.downcast_ref::<CommandFailure>(),
            Some(CommandFailure::Interrupted {
                step: InterruptionStep::Run
            })
        )));
        assert!(!format!("{:#}", failure.error).contains("private hook runner panic payload"));
        assert_eq!(failure.completed().outputs().len(), 1);
        assert_eq!(
            failure.completed().outputs()[0].logical_path(),
            root.join("generated/complete.txt")
        );
        assert!(failure.completed().outputs()[0].is_current());
        let partial = failure.partial_outputs().unwrap();
        assert!(partial.is_current());
        assert!(partial.covers_path(&root.join("generated/partial.txt")));
        assert!(!partial.covers_path(&root.join("generated/complete.txt")));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
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
        config.hooks.before_build.push(hook_named(
            "record",
            "build::tests::child_records_hook_environment",
        ));
        config
            .hooks
            .after_publish
            .push(crate::config::section::build::AfterPublishHookConfig {
                name: "record".into(),
                argv: Some(hook_child_command(
                    "build::tests::child_records_hook_environment",
                )),
                dev: crate::config::section::build::DevParticipation::Run,
                ..Default::default()
            });

        for (mode, expected) in [
            (BuildMode::Development, "dev"),
            (BuildMode::Production, "prod"),
        ] {
            run_before_build_hooks_with_cancellation(
                &config,
                mode,
                tola_input::InputScope::Online,
                &mut PreparedHooks::default(),
                None,
                &mut Vec::new(),
                &HookExecutor::new(),
            )
            .unwrap();
            assert_eq!(
                crate::build::tests::recorded_build_mode(&root),
                expected,
                "before-build {expected}"
            );
            run_after_publish_hooks(
                &config,
                mode,
                tola_input::InputScope::Online,
                &config.build.publish_dir,
                &BuildCancellation::new(),
                &HookExecutor::new(),
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
            config.hooks.before_build = vec![hook_named(
                "images",
                "build::tests::child_records_hook_environment",
            )];
            run_before_build_hooks_with_cancellation(
                &config,
                BuildMode::Production,
                tola_input::InputScope::Online,
                &mut PreparedHooks::default(),
                None,
                &mut Vec::new(),
                &HookExecutor::new(),
            )
            .unwrap();
            let marker =
                crate::build::tests::recorded_hook_cache_directory(&root).join("marker.txt");
            fs::write(&marker, "kept").unwrap();

            match ending {
                Ending::Failed => {
                    config.hooks.before_build = vec![hook_named(
                        "images",
                        "hooks::runner::tests::child_writes_partial_then_fails",
                    )];
                    let error = run_before_build_hooks_with_cancellation(
                        &config,
                        BuildMode::Production,
                        tola_input::InputScope::Online,
                        &mut PreparedHooks::default(),
                        None,
                        &mut Vec::new(),
                        &HookExecutor::new(),
                    )
                    .unwrap_err();
                    assert!(
                        tola_diagnostic::attached(&error.error).is_some(),
                        "{ending:?}"
                    );
                }
                Ending::Cancelled => {
                    config.hooks.before_build = vec![hook_named(
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
                                tola_input::InputScope::Online,
                                &mut PreparedHooks::default(),
                                Some(&cancellation),
                                &mut Vec::new(),
                                &HookExecutor::new(),
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
