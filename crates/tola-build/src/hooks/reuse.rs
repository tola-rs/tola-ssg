//! Reuse of one `generate-outputs` hook's previous development outputs.
//!
//! Tola already identifies file contents with [`ContentDigest`], so reuse compares a
//! structured observation directly instead of inventing a second digest: the built site's
//! output identities, the declared `rerun-on` contents, the command, and the declared
//! outputs. A command that reads time, the network, or another unobservable input must
//! not opt into reuse.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::section::build::hooks::{CommandOutput, OutputCommandConfig};
use crate::output::graph::{OutputFile, OutputKind};
use crate::output::owner::OutputRootOwnership;
use tola_address::OutputPath;
use tola_typst::ContentDigest;

/// Everything Tola can observe about one `generate-outputs` command's inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookInputs {
    command: Vec<String>,
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
    state: RerunOnState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RerunOnState {
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
        outputs,
        upstream,
        rerun_on,
    })
}

/// Observe one declared `rerun-on` path without following symlinks or ignore files.
///
/// A missing path is a state of its own: adding or removing it invalidates reuse.
fn observe_rerun_path(site_root: &Path, relative: &Path) -> Result<RerunOnPath> {
    let path = site_root.join(relative);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(RerunOnPath {
                path: relative.to_path_buf(),
                state: RerunOnState::Missing,
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
        RerunOnState::Symlink(target)
    } else if metadata.is_dir() {
        let mut entries = std::fs::read_dir(&path)
            .with_context(|| {
                format!(
                    "Tola could not read the `rerun-on` directory `{}`",
                    relative.display()
                )
            })?
            .collect::<std::io::Result<Vec<_>>>()
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
        RerunOnState::Directory(children)
    } else if metadata.is_file() {
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "Tola could not read the `rerun-on` file `{}`",
                relative.display()
            )
        })?;
        RerunOnState::File(ContentDigest::of(&bytes))
    } else {
        RerunOnState::Other
    };

    Ok(RerunOnPath {
        path: relative.to_path_buf(),
        state,
    })
}
