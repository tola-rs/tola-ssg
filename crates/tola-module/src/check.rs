//! Check the files explicitly promised by a standalone module manifest.

use std::path::Path;

use anyhow::{Context as _, Result};
use tola_input::{InputMode, InputScope};

use crate::Action;
use crate::declaration::CommandAction;
use crate::manifest::{ReadManifest, read_one};
use crate::module_set::validate_module_path;

/// Read and check a module directory without loading or executing its programs.
pub fn check(directory: &Path) -> Result<()> {
    let loaded = read(directory)?;
    let mut visited = std::collections::BTreeSet::from([loaded.root.clone()]);
    check_loaded_with_visited(&loaded, &mut visited)
}

pub(crate) fn check_loaded(loaded: &ReadManifest) -> Result<()> {
    let mut visited = std::collections::BTreeSet::from([loaded.root.clone()]);
    check_loaded_with_visited(loaded, &mut visited)
}

fn check_loaded_with_visited(
    loaded: &ReadManifest,
    visited: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> Result<()> {
    let manifest = &loaded.manifest;
    let root = &loaded.root;
    if let Some(pack) = &manifest.pack {
        crate::pack::validate_selection(pack)?;
    }
    let file = |key: &str, path: &Path| check_path(root, key, path, false);
    let dir = |key: &str, path: &Path| check_path(root, key, path, true);
    if let Some(typst) = &manifest.typst {
        if let Some(entry) = &typst.entry {
            file("typst.entry", entry)?;
        }
        for (idx, path) in typst.fonts.iter().enumerate() {
            dir(&format!("typst.fonts[{idx}]"), path)?;
        }
    }
    for (key, entry) in [("nu.entry", &manifest.nu), ("build.entry", &manifest.build)] {
        if let Some(entry) = entry {
            file(key, &entry.entry)?;
        }
    }
    for (name, query) in &manifest.queries {
        file(&format!("queries.{name}.entry"), &query.entry)?;
    }
    for (name, src) in &manifest.icons.collections {
        if let Some(path) = src.path() {
            let is_dir = matches!(src, crate::icons::IconCollectionSource::LocalSvgDir { .. });
            check_path(
                root,
                &format!("icons.collections.{name}.path"),
                path,
                is_dir,
            )?;
        }
    }
    if let Some(assets) = &manifest.assets {
        for (idx, asset) in assets.files.iter().enumerate() {
            file(&format!("assets.files[{idx}].source"), &asset.source)?;
        }
        for (idx, asset) in assets.trees.iter().enumerate() {
            dir(&format!("assets.trees[{idx}].source"), &asset.source)?;
        }
    }
    for (name, cmd) in &manifest.commands {
        match cmd.action()? {
            CommandAction::Execute(action) => {
                check_action(root, &format!("commands.{name}.nu"), &action)?;
            }
            CommandAction::Skill { path } => file(&format!("commands.{name}.skill"), &path)?,
            CommandAction::Query { .. } => {}
        }
    }
    if let Some(init) = &manifest.init {
        check_action(root, "init.nu", &init.action)?;
    }
    for (stage, name, path) in manifest.hooks.runtime_scripts() {
        file(&format!("hooks.{} ({name}).nu", stage.as_str()), path)?;
    }
    for hook in &manifest.hooks.before_pack {
        let key = format!("hooks.before-pack ({}).nu", hook.name);
        let action = hook.action().map_err(anyhow::Error::msg)?;
        check_action(root, &key, &action)?;
    }
    check_children(root, manifest, visited)?;
    Ok(())
}

fn check_children(
    root: &Path,
    manifest: &crate::manifest::Manifest,
    visited: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> Result<()> {
    for (alias, child) in &manifest.modules {
        match &child.source {
            crate::ModuleSource::Local { path } => {
                check_path(root, &format!("modules.{alias}.path"), path, true)?;
                let child_root = root.join(path).canonicalize()?;
                anyhow::ensure!(
                    visited.insert(child_root.clone()),
                    "`tola-module.toml` `modules.{alias}` creates a module cycle"
                );
                let child_loaded = read(&child_root)?;
                check_loaded_with_visited(&child_loaded, visited)?;
                visited.remove(&child_root);
            }
            crate::ModuleSource::Builtin { builtin } => {
                anyhow::ensure!(
                    crate::BuiltinModule::named(builtin).is_some(),
                    "`tola-module.toml` `modules.{alias}.builtin` names unknown built-in module `{builtin}`"
                );
            }
            crate::ModuleSource::Git { .. } | crate::ModuleSource::Tarball { .. } => {}
        }
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
        &[],
    )
    .map_err(|err| {
        let diag = crate::diagnostic::manifest_diagnostic(&err);
        let err = anyhow::Error::new(err);
        match diag {
            Some(diag) => tola_diagnostic::DiagnosticError::attach(err, vec![diag]).into(),
            None => err,
        }
    })
}

pub(crate) fn check_action(root: &Path, key: &str, action: &Action) -> Result<()> {
    if let Action::Nu { path, .. } = action {
        check_path(root, key, path, false)?;
    }
    Ok(())
}

fn check_path(root: &Path, key: &str, path: &Path, is_dir: bool) -> Result<()> {
    validate_module_path(root, "module", root, path, is_dir)
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("`tola-module.toml` `{key}`: {err}"))
}
