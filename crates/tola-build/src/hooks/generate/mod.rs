//! Hook-generated files added to the candidate output graph.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tola_address::OutputPath;
use tola_input::layout;

use super::command::{HookCall, HookDirectories, HookSitePaths, ObserveFailure, run_action};
use super::report::{
    BatchFailure, failed_to_start, genuinely_failed, hook_command_error, report_reused,
    run_hook_entry,
};
use super::reuse::{HookId, ReusableHookOutput, ReusableHookOutputs, observe};
use super::{HookExecutor, participating_batches};
use crate::cancellation::BuildCancellation;
use crate::config::SiteConfig;
use crate::config::section::build::hooks::{CommandOutput, HookStage, OutputCommandConfig};
use crate::filesystem::TemporaryDirectory;
use crate::mode::BuildMode;
use crate::output::files::HookOutputFiles;
use crate::output::graph::OutputFile;
use crate::output::owner::{OutputOwner, OutputRootOwnership};
use crate::output::semantics::OutputDeclaration;

#[derive(Debug, Default)]
pub(crate) struct GeneratedOutputs {
    pub(crate) outputs: Vec<OutputFile>,
    pub(crate) root_ownerships: Vec<OutputRootOwnership>,
    pub(crate) reuse: ReusableHookOutputs,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_outputs(
    config: &SiteConfig,
    upstream: &[OutputFile],
    mode: BuildMode,
    scope: tola_input::InputScope,
    previous: Option<&ReusableHookOutputs>,
    cancellation: &BuildCancellation,
    diagnostics: &mut Vec<tola_diagnostic::Diagnostic>,
    executor: &HookExecutor,
) -> Result<GeneratedOutputs> {
    cancellation.ensure_active()?;
    let stage = HookStage::GenerateOutputs;
    let hooks = &config.hooks.generate_outputs;
    let mut candidate = None;
    let mut generated = GeneratedOutputs::default();
    for batch in participating_batches(hooks, false, mode, |h| {
        (
            h.origin.as_ref().map(|o| o.name.as_str()),
            h.order,
            h.enable,
            h.dev,
        )
    }) {
        cancellation.ensure_active()?;
        let mut entries = Vec::with_capacity(batch.len());
        let mut ready = Vec::new();
        for (member, &idx) in batch.iter().enumerate() {
            cancellation.ensure_active()?;
            let hook = &hooks[idx];
            let id = HookId::new(
                hook.origin.as_ref(),
                &hook.name,
                hook.declared_name.as_deref(),
            );
            let inputs = (mode == BuildMode::Development && hook.dev.reuses_outputs())
                .then(|| observe(config, hook, upstream, scope, cancellation))
                .transpose()?;
            let cached = inputs
                .as_ref()
                .zip(previous)
                .and_then(|(inputs, prev)| prev.matching(&id, inputs));
            if cached.is_none() {
                ready.push(member);
            }
            entries.push((id, inputs, cached));
        }
        let mut results = if ready.is_empty() {
            Vec::new()
        } else {
            if candidate.is_none() {
                candidate = Some(HookOutputFiles::materialize_outputs(
                    config.root(),
                    upstream,
                    Some(cancellation),
                )?);
            }
            let candidate = candidate.as_ref().expect("materialized above");
            executor.run_batch(
                ready.len(),
                Some(cancellation),
                |slot, stop, observe_failure| {
                    let idx = batch[ready[slot]];
                    let hook = &hooks[idx];
                    let mut report = Vec::new();
                    let res = run_hook_entry(stage, &hook.name, stop, || {
                        stop.ensure_active()?;
                        let res = generate_entry(GenerateEntry {
                            cmd: hook,
                            index: idx,
                            candidate,
                            cfg: config,
                            mode,
                            scope,
                            cancel: stop,
                            diags: &mut report,
                            observe_failure,
                        });
                        if genuinely_failed(&res) {
                            observe_failure();
                        }
                        res
                    });
                    (res, report)
                },
                |(res, _)| genuinely_failed(res),
            )
        }
        .into_iter();
        let mut failure = BatchFailure::default();
        for (member, (id, inputs, cached)) in entries.into_iter().enumerate() {
            let idx = batch[member];
            let name = hooks[idx].name.as_str();
            let (outputs, roots, report) = if let Some(cached) = cached {
                let owner = OutputOwner::command(idx, name);
                let outputs = cached
                    .outputs
                    .iter()
                    .map(|out| out.with_owner(owner.clone()))
                    .collect();
                let roots = cached
                    .roots
                    .iter()
                    .map(|root| OutputRootOwnership::new(root.root().clone(), owner.clone()))
                    .collect();
                report_reused(stage, name);
                (outputs, roots, cached.report.clone())
            } else {
                let res = results.next().expect("each uncached hook is in the batch");
                match res {
                    Some(Ok((Ok((outputs, roots)), report))) => (outputs, roots, report),
                    Some(Ok((Err(err), report))) => {
                        diagnostics.extend(report);
                        failure.record(err, diagnostics);
                        continue;
                    }
                    Some(Err(err)) => {
                        failure.record(failed_to_start(stage, name, err), diagnostics);
                        continue;
                    }
                    None => continue,
                }
            };
            diagnostics.extend(report.iter().cloned());
            if let Some(inputs) = inputs {
                generated.reuse.insert(
                    id,
                    ReusableHookOutput {
                        inputs,
                        outputs: outputs.clone(),
                        roots: roots.clone(),
                        report,
                    },
                );
            }
            generated.outputs.extend(outputs);
            generated.root_ownerships.extend(roots);
        }
        if let Some(err) = failure.error() {
            return Err(err);
        }
    }
    cancellation.ensure_active()?;
    Ok(generated)
}

struct GenerateEntry<'a> {
    cmd: &'a OutputCommandConfig,
    index: usize,
    candidate: &'a HookOutputFiles,
    cfg: &'a SiteConfig,
    mode: BuildMode,
    scope: tola_input::InputScope,
    cancel: &'a BuildCancellation,
    diags: &'a mut Vec<tola_diagnostic::Diagnostic>,
    observe_failure: &'a ObserveFailure,
}

fn generate_entry(entry: GenerateEntry<'_>) -> Result<(Vec<OutputFile>, Vec<OutputRootOwnership>)> {
    let GenerateEntry {
        cmd,
        index,
        candidate,
        cfg,
        mode,
        scope,
        cancel,
        diags,
        observe_failure,
    } = entry;
    if cmd.outputs.is_empty() {
        return Err(super::hook_contract_error(
            format!("`hooks.generate-outputs[{index}].outputs` is empty"),
            "declare each output with `{ file = … }` or `{ tree = … }`",
        ));
    }
    let generated = TemporaryDirectory::create(&layout::hook_outputs(cfg.root()), "outputs")?;
    let identity = super::hook_identity(HookStage::GenerateOutputs, &cmd.name);
    run_action(
        cfg,
        cmd.action(),
        HookCall {
            site: HookSitePaths::new(cfg, scope),
            name: &cmd.name,
            origin: cmd.origin.as_ref(),
            mode,
            directories: HookDirectories::GenerateOutputs {
                input: candidate.root(),
                output: generated.path(),
                outputs: &cmd.outputs,
            },
        },
        &cmd.rerun_on,
        Some(cancel),
        diags,
        observe_failure,
    )
    .map_err(|err| hook_command_error(HookStage::GenerateOutputs, &identity, err))?;
    let res = collect_entry_outputs(cmd, index, generated.path(), &identity, cancel);
    if genuinely_failed(&res) {
        observe_failure();
    }
    res
}

fn collect_entry_outputs(
    cmd: &OutputCommandConfig,
    index: usize,
    generated: &Path,
    identity: &str,
    cancel: &BuildCancellation,
) -> Result<(Vec<OutputFile>, Vec<OutputRootOwnership>)> {
    let files = read_generated_files(generated, identity, cancel)
        .with_context(|| format!("Tola could not read the files written by {identity}"))?;
    let decls = DeclaredOutputs::new(&cmd.outputs, cancel)?;
    for path in files.regular.keys() {
        cancel.ensure_active()?;
        if !decls.contains(path) {
            return Err(super::hook_contract_error(
                format!("{identity} produced `{path}`, which `outputs` does not declare"),
                "declare it in `outputs`, or stop writing it",
            ));
        }
    }
    for decl in &cmd.outputs {
        cancel.ensure_active()?;
        match decl {
            CommandOutput::File(path) if !files.regular.contains_key(path) => {
                return Err(super::hook_contract_error(
                    format!(
                        "`outputs` of {identity} declares `{path}`, but the command wrote nothing"
                    ),
                    "write it, or remove it from `outputs`",
                ));
            }
            CommandOutput::Tree(root) if !files.directories.contains(Path::new(root.as_str())) => {
                return Err(super::hook_contract_error(
                    format!(
                        "`outputs` of {identity} declares the directory `{root}`, but the command created none"
                    ),
                    "create it, or declare the files instead",
                ));
            }
            _ => {}
        }
    }
    let owner = OutputOwner::command(index, cmd.name.as_str());
    let roots = cmd
        .outputs
        .iter()
        .filter_map(|decl| match decl {
            CommandOutput::Tree(root) => {
                Some(OutputRootOwnership::new(root.clone(), owner.clone()))
            }
            CommandOutput::File(_) => None,
        })
        .collect();
    let mut outputs = Vec::with_capacity(files.regular.len());
    for (path, bytes) in files.regular {
        cancel.ensure_active()?;
        let media = OutputDeclaration::from_filesystem_source(Path::new(path.as_str()))
            .media_type()
            .clone();
        outputs.push(OutputFile::new(
            path,
            OutputDeclaration::opaque(media),
            owner.clone(),
            bytes,
        ));
    }
    Ok((outputs, roots))
}

struct DeclaredOutputs<'a> {
    files: BTreeSet<&'a OutputPath>,
    trees: Vec<String>,
}

impl<'a> DeclaredOutputs<'a> {
    fn new(decls: &'a [CommandOutput], cancel: &BuildCancellation) -> Result<Self> {
        let mut files = BTreeSet::new();
        let mut sorted_trees = BTreeSet::new();
        for decl in decls {
            cancel.ensure_active()?;
            match decl {
                CommandOutput::File(path) => {
                    files.insert(path);
                }
                CommandOutput::Tree(root) => {
                    sorted_trees.insert(format!("{}/", root.as_str()));
                }
            }
        }
        let mut trees: Vec<String> = Vec::new();
        for root in sorted_trees {
            cancel.ensure_active()?;
            if !trees.last().is_some_and(|parent| root.starts_with(parent)) {
                trees.push(root);
            }
        }
        Ok(Self { files, trees })
    }

    fn contains(&self, path: &OutputPath) -> bool {
        if self.files.contains(path) {
            return true;
        }
        let end = self
            .trees
            .partition_point(|root| root.as_str() <= path.as_str());
        end.checked_sub(1)
            .is_some_and(|idx| path.as_str().starts_with(&self.trees[idx]))
    }
}

#[derive(Default)]
struct GeneratedFiles {
    regular: BTreeMap<OutputPath, Vec<u8>>,
    directories: BTreeSet<PathBuf>,
}

fn read_generated_files(
    root: &Path,
    identity: &str,
    cancel: &BuildCancellation,
) -> Result<GeneratedFiles> {
    cancel.ensure_active()?;
    if !fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir() && !tola_fs::is_link_like(&meta))
    {
        return Err(super::hook_contract_error(
            format!("{identity} did not leave a regular output directory"),
            "write files inside the hook context output directory",
        ));
    }
    let mut files = GeneratedFiles::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        cancel.ensure_active()?;
        for child in crate::filesystem::read_sorted_entries(&dir, cancel, root, "generated")? {
            cancel.ensure_active()?;
            let abs = child.path();
            let rel = abs
                .strip_prefix(root)
                .expect("generated child remains below its root");
            let shown = tola_diagnostic::display_path(&abs, root);
            let meta = fs::symlink_metadata(&abs)
                .with_context(|| format!("Tola could not read the generated file `{shown}`"))?;
            if tola_fs::is_link_like(&meta) {
                return Err(super::hook_contract_error(
                    format!("the command generated `{shown}` as a symbolic link"),
                    "generate a regular file or directory",
                ));
            }
            if meta.is_dir() {
                files.directories.insert(rel.to_path_buf());
                pending.push(abs);
            } else if meta.is_file() {
                let path = rel
                    .components()
                    .map(|c| {
                        c.as_os_str().to_str().ok_or_else(|| {
                            super::hook_contract_error(
                                format!(
                                    "the command generated `{shown}`, whose name is not valid UTF-8"
                                ),
                                "rename it to valid UTF-8",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                let path = OutputPath::parse(&path).map_err(|err| {
                    let help = crate::output::path::invalid_path_help(&err)
                        .unwrap_or("rename the generated file");
                    super::hook_contract_error(
                        format!(
                            "{identity} generated an invalid output path `{}`: {err}",
                            path.escape_debug()
                        ),
                        help,
                    )
                    .context(err)
                })?;
                let mut buf = Vec::new();
                tola_input::read_file_chunks(&abs, cancel.flag(), |chunk| {
                    buf.extend_from_slice(chunk);
                })
                .map_err(crate::filesystem::FileReadErrorExt::into_anyhow)
                .with_context(|| format!("Tola could not read the generated file `{path}`"))?;
                files.regular.insert(path, buf);
            } else {
                return Err(super::hook_contract_error(
                    format!(
                        "the command generated `{shown}`, which is not a regular file or directory"
                    ),
                    "generate a regular file or directory",
                ));
            }
        }
    }
    Ok(files)
}
