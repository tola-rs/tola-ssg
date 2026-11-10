//! Development reuse of hook work: one `generate-outputs` hook's previous outputs, and the
//! outcome of the latest after-publish run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
use tola_diagnostic::display_path;
use tola_module::hooks::RerunOn;
use tola_typst::ContentDigest;

/// Everything Tola can observe about one `generate-outputs` command's inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookInputs {
    action: tola_module::Action,
    nu: Option<NuInputs>,
    origin: Option<ModuleHookOrigin>,
    outputs: Vec<CommandOutput>,
    upstream: Vec<UpstreamOutput>,
    rerun_on: RerunOn,
    paths: BTreeMap<PathBuf, ObservedPath>,
}

/// Values an embedded Nu hook can read without touching the filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NuInputs {
    values: ContentDigest,
    assets: Option<std::sync::Arc<crate::asset::AssetUrls>>,
}

impl NuInputs {
    fn observe(config: &ResolvedSiteConfig) -> Result<Self> {
        let runner = config.hook_runner.as_ref();
        let modules = runner.and_then(|runner| runner.modules()).map(|prepared| {
            let instances = prepared
                .modules
                .iter()
                .map(|(alias, module)| (alias, &module.root, &module.manifest, &module.options))
                .collect::<Vec<_>>();
            let asset_modules = prepared
                .assets
                .iter()
                .map(|asset| &asset.alias)
                .collect::<Vec<_>>();
            (instances, asset_modules)
        });
        let bytes = serde_json::to_vec(&(
            config.site(),
            config.options(),
            config.config_path(),
            &config.build().content_dir,
            modules,
        ))
        .context("read the configuration available to Nu hooks")?;
        Ok(Self {
            values: ContentDigest::of(&bytes),
            assets: runner.and_then(|runner| runner.assets()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UpstreamOutput {
    path: OutputPath,
    kind: OutputKind,
    digest: ContentDigest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ObservedPath {
    Missing,
    File(blake3::Hash),
    Directory,
    Symlink(PathBuf),
    Other,
}

/// The outputs one hook produced, valid while its observed inputs match.
#[derive(Clone, Debug)]
pub(crate) struct ReusableHookOutput {
    pub(crate) inputs: HookInputs,
    pub(crate) outputs: Vec<OutputFile>,
    pub(crate) roots: Vec<OutputRootOwnership>,
    pub(crate) report: Vec<tola_diagnostic::Diagnostic>,
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
    config: &ResolvedSiteConfig,
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

    let action = command.action().map_err(anyhow::Error::msg)?;
    let paths = observe_paths(
        config.get_root(),
        command.origin.as_ref(),
        &command.rerun_on,
        command.nu.as_deref(),
    )?;

    Ok(HookInputs {
        nu: command
            .nu
            .as_ref()
            .map(|_| NuInputs::observe(config))
            .transpose()?,
        action,
        origin: command.origin.clone(),
        outputs,
        upstream,
        rerun_on: command.rerun_on.clone(),
        paths,
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
    nu: Option<NuInputs>,
    hooks: Vec<AfterPublishHookConfig>,
    paths: Vec<BTreeMap<PathBuf, ObservedPath>>,
}

pub(crate) fn observe_after_publish(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    revision: &RevisionId,
) -> Result<AfterPublishDependencies> {
    let hooks: Vec<AfterPublishHookConfig> = config
        .hooks
        .after_publish
        .iter()
        .filter(|hook| super::hook_participates(hook.enable, hook.dev, mode))
        .cloned()
        .collect();
    let paths = hooks
        .iter()
        .map(|hook| {
            observe_paths(
                config.get_root(),
                hook.origin.as_ref(),
                &hook.rerun_on,
                hook.nu.as_deref(),
            )
        })
        .collect::<Result<_>>()?;
    let nu = hooks
        .iter()
        .any(|hook| hook.nu.is_some())
        .then(|| NuInputs::observe(config))
        .transpose()?;
    Ok(AfterPublishDependencies {
        revision: revision.clone(),
        nu,
        hooks,
        paths,
    })
}

/// Record selected paths in path order, including missing literals and the Nu entry.
fn observe_paths(
    root: &Path,
    origin: Option<&ModuleHookOrigin>,
    rerun_on: &RerunOn,
    nu: Option<&Path>,
) -> Result<BTreeMap<PathBuf, ObservedPath>> {
    let selection = rerun_on.compile(root, origin)?;
    let nu = nu
        .map(|path| super::command::nu_script(root, origin, path))
        .transpose()?
        .map(|path| tola_input::normalize_existing_prefix(&path));
    let literals: BTreeSet<_> = selection.literal_includes().collect();
    // Parents sort before children, so overlapping includes visit a path once.
    let mut pending: BTreeSet<_> = selection
        .scoped_roots()
        .filter(|(scope, path)| {
            !selection.excludes(path)
                && (literals.contains(path) || !prefix_crosses_link(scope, path))
        })
        .map(|(_, path)| path)
        .collect();
    pending.extend(nu.iter().cloned());
    let mut observed = BTreeMap::new();
    while let Some(path) = pending.pop_first() {
        let script = nu.as_deref() == Some(path.as_path());
        let selected = selection.matches(&path);
        let descend = selected || selection.could_match_below(&path);
        if !descend && !script {
            continue;
        }
        let unreadable = || {
            let path = display_path(&path, root);
            let key = if script { "nu" } else { "rerun-on" };
            format!("Tola could not read the `{key}` path `{path}`")
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
            {
                if script || (selected && literals.contains(&path)) {
                    observed.insert(path, ObservedPath::Missing);
                }
                continue;
            }
            Err(error) => return Err(error).with_context(unreadable),
        };
        if descend && metadata.is_dir() {
            for entry in fs::read_dir(&path).with_context(unreadable)? {
                pending.insert(entry.with_context(unreadable)?.path());
            }
        }
        if script || selected {
            let state = if metadata.file_type().is_symlink() {
                ObservedPath::Symlink(fs::read_link(&path).with_context(unreadable)?)
            } else if metadata.is_dir() {
                ObservedPath::Directory
            } else if metadata.is_file() {
                let digest =
                    crate::filesystem::digest_file(&path, None).with_context(unreadable)?;
                ObservedPath::File(digest)
            } else {
                ObservedPath::Other
            };
            observed.insert(path, state);
        }
    }
    Ok(observed)
}

// A glob can start below its scope without walking its fixed prefix. Check that
// prefix for links so the shortcut cannot enter a directory the walk would skip.
fn prefix_crosses_link(scope: &Path, prefix: &Path) -> bool {
    prefix
        .ancestors()
        .skip(1)
        .take_while(|path| *path != scope && path.starts_with(scope))
        .any(|path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_changes_invalidate_outputs() {
        let root = tempfile::tempdir().unwrap();
        let mut hook = OutputCommandConfig {
            argv: Some(vec!["program".into()]),
            origin: Some(ModuleHookOrigin {
                name: "notes".into(),
                root: root.path().join("module"),
                manifest: root.path().join("module/tola-module.toml"),
                options_json: br#"{"search":true}"#.to_vec(),
            }),
            ..Default::default()
        };
        let config = crate::config::tests::load_test_config(root.path(), "");
        let before = observe(&config, &hook, &[]).unwrap();
        hook.origin.as_mut().unwrap().options_json = br#"{"search":false}"#.to_vec();
        let after = observe(&config, &hook, &[]).unwrap();
        assert_ne!(before, after);
    }
}
