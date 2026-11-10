//! Check the files explicitly promised by a standalone module manifest.

use std::path::Path;

use anyhow::{Context as _, Result};
use tola_input::{InputMode, InputScope};

use crate::Action;
use crate::declaration::CommandAction;
use crate::manifest::{ManifestError, ReadManifest, read_one};
use crate::module_set::validate_module_path;

/// Read and check a module directory without loading or executing its programs.
pub fn check(directory: &Path) -> Result<()> {
    let loaded = read(directory)?;
    check_loaded(&loaded)
}

pub(crate) fn check_loaded(loaded: &ReadManifest) -> Result<()> {
    let manifest = &loaded.manifest;
    if let Some(pack) = &manifest.pack {
        crate::pack::validate_selection(pack)?;
    }
    let file = |key: &str, path: &Path| check_path(&loaded.root, key, path, false);
    for (key, entry) in [
        ("typst.entry", &manifest.typst),
        ("nu.entry", &manifest.nu),
        ("build.entry", &manifest.build),
    ] {
        if let Some(entry) = entry {
            file(key, &entry.entry)?;
        }
    }
    for (name, query) in &manifest.queries {
        file(&format!("queries.{name}.entry"), &query.entry)?;
    }
    if let Some(assets) = &manifest.assets {
        for (index, asset) in assets.files.iter().enumerate() {
            file(&format!("assets.files[{index}].source"), &asset.source)?;
        }
        for (index, asset) in assets.trees.iter().enumerate() {
            check_path(
                &loaded.root,
                &format!("assets.trees[{index}].source"),
                &asset.source,
                true,
            )?;
        }
    }
    for (name, command) in &manifest.commands {
        match command.action()? {
            CommandAction::Execute(action) => {
                check_action(&loaded.root, &format!("commands.{name}.nu"), &action)?;
            }
            CommandAction::Skill { path } => file(&format!("commands.{name}.skill"), &path)?,
            CommandAction::Query { .. } => {}
        }
    }
    if let Some(init) = &manifest.init {
        check_action(&loaded.root, "init.nu", &init.action)?;
    }
    for (stage, name, path) in manifest.hooks.runtime_scripts() {
        file(&format!("hooks.{} ({name}).nu", stage.as_str()), path)?;
    }
    for hook in &manifest.hooks.before_pack {
        let key = format!("hooks.before-pack ({}).nu", hook.name);
        let action = hook.action().map_err(anyhow::Error::msg)?;
        check_action(&loaded.root, &key, &action)?;
    }
    Ok(())
}

pub(crate) fn read(directory: &Path) -> Result<ReadManifest> {
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    read_one(
        &cwd,
        "module",
        cwd.join(directory),
        InputMode::new(InputScope::Online, false),
    )
    .map_err(|error| match error {
        ManifestError::ManifestParse { path, source } => {
            anyhow::anyhow!("`{path}`: {}", source.message())
        }
        error => error.into(),
    })
}

pub(crate) fn check_action(root: &Path, key: &str, action: &Action) -> Result<()> {
    if let Action::Nu { path, .. } = action {
        check_path(root, key, path, false)?;
    }
    Ok(())
}

fn check_path(root: &Path, key: &str, path: &Path, directory: bool) -> Result<()> {
    validate_module_path(root, "module", root, path, directory)
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("`tola-module.toml` `{key}`: {error}"))
}
