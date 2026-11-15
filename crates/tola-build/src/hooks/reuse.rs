//! Development reuse of hook work: one `generate-outputs` hook's previous outputs, and the
//! outcome of the latest after-publish run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::hooks::{CommandOutput, ModuleHookOrigin, OutputCommandConfig};
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
            runner
                .and_then(|runner| runner.resources())
                .map(|resources| config.compiler_font_options(resources)),
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

/// The hook's declaration identity within a stage.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct HookId {
    module: Option<String>,
    name: String,
}

impl HookId {
    pub(super) fn new(
        origin: Option<&ModuleHookOrigin>,
        name: &str,
        declared_name: Option<&str>,
    ) -> Self {
        Self {
            module: origin.map(|origin| origin.name.clone()),
            name: declared_name.unwrap_or(name).to_owned(),
        }
    }
}

/// Previous development revision's reusable outputs, one entry per hook.
#[derive(Clone, Default, Debug)]
pub(crate) struct ReusableHookOutputs {
    entries: BTreeMap<HookId, ReusableHookOutput>,
}

impl ReusableHookOutputs {
    /// The previous outputs for a hook when their observed inputs still match.
    pub(super) fn matching(&self, id: &HookId, inputs: &HookInputs) -> Option<&ReusableHookOutput> {
        self.entries.get(id).filter(|entry| entry.inputs == *inputs)
    }

    pub(super) fn insert(&mut self, id: HookId, output: ReusableHookOutput) {
        self.entries.insert(id, output);
    }
}

/// Observe the inputs a pure `generate-outputs` command may depend on.
pub(crate) fn observe(
    config: &ResolvedSiteConfig,
    command: &OutputCommandConfig,
    upstream: &[OutputFile],
    cancellation: &BuildCancellation,
) -> Result<HookInputs> {
    cancellation.ensure_active()?;
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
        cancellation,
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
/// revision, the effective order batches and the participating commands' observed inputs.
///
/// Equal dependencies give the commands nothing new to consume, so development keeps the
/// previous run's outcome instead of materializing the site and running them again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AfterPublishDependencies {
    revision: RevisionId,
    nu: Option<NuInputs>,
    batches: Vec<Vec<AfterPublishInputs>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AfterPublishInputs {
    id: HookId,
    action: tola_module::Action,
    origin: Option<ModuleHookOrigin>,
    rerun_on: RerunOn,
    paths: BTreeMap<PathBuf, ObservedPath>,
}

pub(crate) fn observe_after_publish(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    revision: &RevisionId,
    cancellation: &BuildCancellation,
) -> Result<AfterPublishDependencies> {
    cancellation.ensure_active()?;
    let mut uses_nu = false;
    let mut batches = Vec::new();
    let hooks = &config.hooks.after_publish;
    for batch in super::runner::after_publish_batches(config, mode) {
        let inputs = batch
            .into_iter()
            .map(|index| {
                let hook = &hooks[index];
                uses_nu |= hook.nu.is_some();
                let paths = observe_paths(
                    config.get_root(),
                    hook.origin.as_ref(),
                    &hook.rerun_on,
                    hook.nu.as_deref(),
                    cancellation,
                )?;
                Ok(AfterPublishInputs {
                    id: HookId::new(
                        hook.origin.as_ref(),
                        &hook.name,
                        hook.declared_name.as_deref(),
                    ),
                    action: hook.action().map_err(anyhow::Error::msg)?,
                    origin: hook.origin.clone(),
                    rerun_on: hook.rerun_on.clone(),
                    paths,
                })
            })
            .collect::<Result<_>>()?;
        batches.push(inputs);
    }
    Ok(AfterPublishDependencies {
        revision: revision.clone(),
        nu: uses_nu.then(|| NuInputs::observe(config)).transpose()?,
        batches,
    })
}

/// Record selected paths in path order, including missing literals and the Nu entry.
fn observe_paths(
    root: &Path,
    origin: Option<&ModuleHookOrigin>,
    rerun_on: &RerunOn,
    nu: Option<&Path>,
    cancellation: &BuildCancellation,
) -> Result<BTreeMap<PathBuf, ObservedPath>> {
    cancellation.ensure_active()?;
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
        cancellation.ensure_active()?;
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
                cancellation.ensure_active()?;
                pending.insert(entry.with_context(unreadable)?.path());
            }
        }
        if script || selected {
            let state = if metadata.file_type().is_symlink() {
                ObservedPath::Symlink(fs::read_link(&path).with_context(unreadable)?)
            } else if metadata.is_dir() {
                ObservedPath::Directory
            } else if metadata.is_file() {
                let digest = crate::filesystem::digest_file(&path, Some(cancellation))
                    .with_context(unreadable)?;
                ObservedPath::File(digest)
            } else {
                ObservedPath::Other
            };
            observed.insert(path, state);
        }
    }
    cancellation.ensure_active()?;
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
    fn after_publish_reuse_depends_on_batches_not_order_numbers() {
        use crate::config::section::build::hooks::{AfterPublishHookConfig, DevParticipation};
        use crate::output::graph::OutputGraphBuilder;
        use crate::output::manifest::SiteManifest;

        let root = tempfile::tempdir().unwrap();
        let mut config = crate::config::tests::load_test_config(root.path(), "");
        config.hooks.after_publish = ["upload", "notify"]
            .into_iter()
            .map(|name| AfterPublishHookConfig {
                name: name.into(),
                argv: Some(vec![name.into()]),
                dev: DevParticipation::Run,
                ..Default::default()
            })
            .collect();
        let manifest = SiteManifest::from_graph(&OutputGraphBuilder::new().finish());
        let revision = manifest.revision();
        let before = observe_after_publish(
            &config,
            BuildMode::Development,
            revision,
            &BuildCancellation::default(),
        )
        .unwrap();
        for hook in &mut config.hooks.after_publish {
            hook.order = 10;
        }
        assert_eq!(
            observe_after_publish(
                &config,
                BuildMode::Development,
                revision,
                &BuildCancellation::default()
            )
            .unwrap(),
            before
        );
        config.hooks.after_publish[1].order = 20;
        assert_ne!(
            observe_after_publish(
                &config,
                BuildMode::Development,
                revision,
                &BuildCancellation::default()
            )
            .unwrap(),
            before
        );
    }

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
        let before = observe(&config, &hook, &[], &BuildCancellation::default()).unwrap();
        hook.origin.as_mut().unwrap().options_json = br#"{"search":false}"#.to_vec();
        let after = observe(&config, &hook, &[], &BuildCancellation::default()).unwrap();
        assert_ne!(before, after);
    }
}
