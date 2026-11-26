//! Runs trusted hook commands at their configured build stages.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::command::{HookCall, HookDirectories, HookSitePaths, ObserveFailure, run_action};
use super::output::{
    FailedHookOutputs, HookOutput, SourceHookOutputs, declared_output_files,
    validate_declared_output_boundaries,
};
use super::report::{
    BatchFailure, failed_to_start, genuinely_failed, hook_command_error, report_reused,
    run_hook_entry,
};
use super::reuse::{BeforeBuildInputs, HookId, ReadyHook, ReadyHooks};
use super::{HookExecutor, hook_participates, participating_batches};
use crate::cancellation::{BuildCancellation, OptionalCancellation};
use crate::config::SiteConfig;
use crate::config::section::build::BeforeBuildHookConfig;
use crate::config::section::build::hooks::HookStage;
use crate::mode::BuildMode;

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
        cancel: Option<&BuildCancellation>,
    ) -> Self {
        let partial = if crate::cancellation::is_cancelled(&error) {
            None
        } else {
            FailedHookOutputs::capture(paths, cancel).ok().flatten()
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
    cfg: &SiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    cancel: Option<&BuildCancellation>,
    executed_outputs: &mut BTreeSet<PathBuf>,
    diags: &mut Vec<tola_diagnostic::Diagnostic>,
    observe_failure: &ObserveFailure,
) -> Result<Vec<HookOutput>> {
    cancel.ensure_active_if_present()?;
    let identity = super::hook_identity(HookStage::BeforeBuild, &hook.name);
    let before = validate_declared_output_boundaries(hook, cfg, &cfg.build.publish_dir)?;
    executed_outputs.extend(before.iter().map(|id| id.logical_path().to_path_buf()));
    run_action(
        cfg,
        hook.action(),
        HookCall {
            site: HookSitePaths::new(cfg, scope),
            name: &hook.name,
            origin: hook.origin.as_ref(),
            mode,
            directories: HookDirectories::BeforeBuild,
        },
        &hook.rerun_on,
        cancel,
        diags,
        observe_failure,
    )
    .map_err(|err| hook_command_error(HookStage::BeforeBuild, &identity, err))?;
    let after = declared_output_files(hook, cfg, cancel)?;
    for files in &after {
        if files.contents.is_none() {
            let declared =
                tola_diagnostic::display_path(files.source_identity.logical_path(), cfg.root());
            return Err(super::hook_contract_error(
                format!("{identity} declares `{declared}`, but the hook wrote nothing"),
                "write it, or remove it from `generates`",
            ));
        }
    }
    after
        .into_iter()
        .map(|files| HookOutput::from_successful_output(files, cancel))
        .collect()
}

struct Preparation {
    outputs: Vec<HookOutput>,
    cache: Option<Arc<ReadyHook>>,
    executed: bool,
}

struct BeforeBuildHooks<'a> {
    cfg: &'a SiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    previous: &'a ReadyHooks,
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
            BeforeBuildInputs::observe(self.cfg, hook, self.scope, stop)?
        } else {
            None
        };
        if let Some(inputs) = &inputs
            && let Some(saved) = self.previous.matching(id, inputs, stop)?
        {
            validate_declared_output_boundaries(hook, self.cfg, &self.cfg.build.publish_dir)?;
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
                self.cfg,
                self.mode,
                self.scope,
                Some(stop),
                started_outputs,
                reported,
                observe_failure,
            )
        })?;
        // Retain outputs only when observed inputs still match after the generator runs.
        let cache = if let Some(before) = inputs
            && BeforeBuildInputs::observe(self.cfg, hook, self.scope, stop)?.as_ref()
                == Some(&before)
        {
            Some(Arc::new(ReadyHook {
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
    config: &SiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    cache: &mut ReadyHooks,
    cancellation: Option<&BuildCancellation>,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
    executor: &HookExecutor,
) -> Result<BeforeBuildExecution, SourceHookFailure> {
    let previous = std::mem::take(cache);
    let commands = BeforeBuildHooks {
        cfg: config,
        mode,
        scope,
        previous: &previous,
    };
    let mut completed = SourceHookOutputs::default();
    let mut hooks_executed = false;
    if let Err(err) = cancellation.ensure_active_if_present() {
        return Err(SourceHookFailure::new(
            err.into(),
            completed,
            &BTreeSet::new(),
            None,
        ));
    }
    let stage = HookStage::BeforeBuild;
    let hooks = &config.hooks.before_build;
    for batch in participating_batches(hooks, true, mode, |h| {
        (
            h.origin.as_ref().map(|o| o.name.as_str()),
            h.order,
            h.enable,
            h.dev,
        )
    }) {
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
        for (member, res) in results.into_iter().enumerate() {
            match res {
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
                        Err(err) => {
                            partial_paths.extend(started_outputs);
                            failure.record(err, diagnostics);
                        }
                    }
                }
                Some(Err(err)) => {
                    let name = &hooks[batch[member]].name;
                    failure.record(failed_to_start(stage, name, err), diagnostics);
                }
                None => {}
            }
        }
        if let Some(err) = failure.error() {
            return Err(SourceHookFailure::new(
                err,
                completed,
                &partial_paths,
                cancellation,
            ));
        }
        if let Err(err) = cancellation.ensure_active_if_present() {
            return Err(SourceHookFailure::new(
                err.into(),
                completed,
                &partial_paths,
                None,
            ));
        }
    }
    if let Err(err) = cancellation.ensure_active_if_present() {
        return Err(SourceHookFailure::new(
            err.into(),
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

pub fn has_after_publish_hooks(config: &SiteConfig, mode: BuildMode) -> bool {
    config
        .hooks
        .after_publish
        .iter()
        .any(|hook| hook_participates(hook.enable, hook.dev, mode))
}

pub(super) fn after_publish_batches(cfg: &SiteConfig, mode: BuildMode) -> Vec<Vec<usize>> {
    participating_batches(&cfg.hooks.after_publish, false, mode, |h| {
        (
            h.origin.as_ref().map(|o| o.name.as_str()),
            h.order,
            h.enable,
            h.dev,
        )
    })
}

/// Consume one committed output view using every participating hook.
pub fn run_after_publish_hooks(
    config: &SiteConfig,
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
    config: &SiteConfig,
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
                    let res = run_action(
                        config,
                        hook.action(),
                        HookCall {
                            site: HookSitePaths::new(config, scope),
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
                    if genuinely_failed(&res) {
                        observe_failure();
                    }
                    res.map_err(|err| hook_command_error(stage, &identity, err))?;
                    Ok(reported)
                })
            },
            genuinely_failed,
        );
        let mut failure = BatchFailure::default();
        let mut callback_err = None;
        for (member, res) in results.into_iter().enumerate() {
            let hook = &hooks[batch[member]];
            let outcome = match res {
                Some(Ok(outcome)) => outcome,
                Some(Err(err)) => Err(failed_to_start(stage, &hook.name, err)),
                None => continue,
            };
            if let Err(err) = completed(&hook.name, &outcome) {
                callback_err.get_or_insert(err);
            }
            match outcome {
                Ok(reported) => diagnostics.extend(reported),
                Err(err) => failure.record(err, &mut diagnostics),
            }
        }
        if let Some(err) = callback_err {
            return Err(err);
        }
        if let Some(err) = failure.error() {
            if diagnostics.is_empty() {
                return Err(err);
            }
            if !crate::cancellation::is_cancelled(&err) {
                diagnostics.extend_from_slice(tola_diagnostic::attached(&err).unwrap_or_default());
            }
            return Err(tola_diagnostic::DiagnosticError::attach(err, diagnostics).into());
        }
    }
    cancellation.ensure_active()?;
    Ok(diagnostics)
}
