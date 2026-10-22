//! Development reuse of hook work: one `generate-outputs` hook's previous outputs, and the
//! outcome of the latest after-publish run.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::ResolvedSiteConfig;
use crate::config::section::build::hooks::{
    AfterPublishHookConfig, CommandOutput, ModuleHookOrigin, OutputCommandConfig,
};
use crate::mode::BuildMode;
use crate::output::graph::{OutputFile, OutputKind};
use crate::output::manifest::RevisionId;
use crate::output::owner::OutputRootOwnership;
use tola_address::OutputPath;
use tola_typst::ContentDigest;

/// Everything Tola can observe about one `generate-outputs` command's inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookInputs {
    command: Vec<String>,
    origin: Option<ModuleHookOrigin>,
    outputs: Vec<CommandOutput>,
    upstream: Vec<UpstreamOutput>,
    rerun_on: Vec<RerunOnPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UpstreamOutput {
    path: OutputPath,
    kind: OutputKind,
    digest: ContentDigest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RerunOnPath {
    path: PathBuf,
    state: RerunOnObservation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RerunOnObservation {
    Missing,
    File(ContentDigest),
    Directory(Vec<RerunOnPath>),
    Symlink(PathBuf),
    Other,
}

/// The outputs one hook produced, valid while its observed inputs match.
#[derive(Clone, Debug)]
pub(crate) struct ReusableHookOutput {
    pub(crate) inputs: HookInputs,
    pub(crate) outputs: Vec<OutputFile>,
    pub(crate) roots: Vec<OutputRootOwnership>,
    pub(crate) report: Option<Vec<tola_diagnostic::Diagnostic>>,
}

/// Previous development revision's reusable outputs, one entry per hook name.
#[derive(Clone, Default, Debug)]
pub(crate) struct ReusableHookOutputs {
    entries: BTreeMap<String, ReusableHookOutput>,
}

impl ReusableHookOutputs {
    /// The previous outputs for `name` when their observed inputs still match.
    pub(crate) fn matching(&self, name: &str, inputs: &HookInputs) -> Option<&ReusableHookOutput> {
        self.entries
            .get(name)
            .filter(|entry| entry.inputs == *inputs)
    }

    pub(crate) fn insert(&mut self, name: &str, output: ReusableHookOutput) {
        self.entries.insert(name.to_owned(), output);
    }
}

/// Observe the inputs a pure `generate-outputs` command may depend on.
pub(crate) fn observe(
    site_root: &Path,
    command: &OutputCommandConfig,
    upstream: &[OutputFile],
) -> Result<HookInputs> {
    let mut upstream: Vec<UpstreamOutput> = upstream
        .iter()
        .map(|output| UpstreamOutput {
            path: output.path().clone(),
            kind: output.kind(),
            digest: output.digest(),
        })
        .collect();
    upstream.sort_unstable_by(|left, right| left.path.cmp(&right.path));

    let mut outputs = command.outputs.clone();
    outputs.sort_unstable_by(|left, right| left.path().cmp(right.path()));

    let mut rerun_on: Vec<RerunOnPath> = command
        .rerun_on
        .iter()
        .map(|path| observe_rerun_path(site_root, path))
        .collect::<Result<_>>()?;
    rerun_on.sort_unstable_by(|left, right| left.path.cmp(&right.path));

    Ok(HookInputs {
        command: command.command.clone(),
        origin: command.origin.clone(),
        outputs,
        upstream,
        rerun_on,
    })
}

/// Everything Tola can observe that one revision's after-publish run consumes: the committed
/// revision, the participating commands in configuration order, and their `rerun-on` contents.
///
/// Equal dependencies give the commands nothing new to consume, so development keeps the
/// previous run's outcome instead of materializing the site and running them again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AfterPublishDependencies {
    revision: RevisionId,
    hooks: Vec<AfterPublishHookConfig>,
    rerun_on: Vec<RerunOnPath>,
}

pub(crate) fn observe_after_publish(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    revision: &RevisionId,
) -> Result<AfterPublishDependencies> {
    let hooks: Vec<AfterPublishHookConfig> = config
        .build
        .hooks
        .after_publish
        .iter()
        .filter(|hook| super::hook_participates(hook.enable, hook.dev, mode))
        .cloned()
        .collect();
    let rerun_on = hooks
        .iter()
        .flat_map(|hook| &hook.rerun_on)
        .map(|path| observe_rerun_path(config.get_root(), path))
        .collect::<Result<_>>()?;
    Ok(AfterPublishDependencies {
        revision: revision.clone(),
        hooks,
        rerun_on,
    })
}

/// Observe one declared `rerun-on` path without following symlinks or ignore files.
fn observe_rerun_path(site_root: &Path, relative: &Path) -> Result<RerunOnPath> {
    let path = site_root.join(relative);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(RerunOnPath {
                path: relative.to_path_buf(),
                state: RerunOnObservation::Missing,
            });
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Tola could not observe the `rerun-on` path `{}`",
                    relative.display()
                )
            });
        }
    };

    let state = if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(&path).with_context(|| {
            format!(
                "Tola could not read the `rerun-on` symlink `{}`",
                relative.display()
            )
        })?;
        RerunOnObservation::Symlink(target)
    } else if metadata.is_dir() {
        let mut entries = std::fs::read_dir(&path)
            .and_then(|entries| entries.collect::<std::io::Result<Vec<_>>>())
            .with_context(|| {
                format!(
                    "Tola could not read the `rerun-on` directory `{}`",
                    relative.display()
                )
            })?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        let mut children = Vec::with_capacity(entries.len());
        for entry in entries {
            children.push(observe_rerun_path(
                site_root,
                &relative.join(entry.file_name()),
            )?);
        }
        RerunOnObservation::Directory(children)
    } else if metadata.is_file() {
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "Tola could not read the `rerun-on` file `{}`",
                relative.display()
            )
        })?;
        RerunOnObservation::File(ContentDigest::of(&bytes))
    } else {
        RerunOnObservation::Other
    };

    Ok(RerunOnPath {
        path: relative.to_path_buf(),
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_changes_invalidate_outputs() {
        let root = tempfile::tempdir().unwrap();
        let mut hook = OutputCommandConfig {
            origin: Some(ModuleHookOrigin {
                name: "notes".into(),
                root: root.path().join("module"),
                manifest: root.path().join("module/tola-module.toml"),
                options_json: br#"{"search":true}"#.to_vec(),
            }),
            ..Default::default()
        };
        let before = observe(root.path(), &hook, &[]).unwrap();
        hook.origin.as_mut().unwrap().options_json = br#"{"search":false}"#.to_vec();
        let after = observe(root.path(), &hook, &[]).unwrap();
        assert_ne!(before, after);
    }
}
