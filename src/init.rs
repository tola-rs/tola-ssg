//! Site initialization module.
//!
//! Creates new site structure with default configuration.

use crate::{config::SiteConfig, utils::git};
use anyhow::{Context, Result, bail};
use std::{fs, path::Path};

/// Files to write ignore patterns to
const IGNORE_FILES: &[&str] = &[".gitignore", ".ignore"];

/// Default config filename
const CONFIG_FILE: &str = "tola.toml";

/// Default site directory structure
const SITE_DIRS: &[&str] = &[
    "content",
    "assets/images",
    "assets/iconfonts",
    "assets/fonts",
    "assets/scripts",
    "assets/styles",
    "templates",
    "utils",
];

/// Create a new site with default structure
pub fn new_site(config: &'static SiteConfig, has_name: bool) -> Result<()> {
    let root = config.get_root();

    // Safety check: if no name was provided (init in current dir),
    // the directory must be completely empty
    if !has_name && !is_dir_empty(root)? {
        bail!(
            "Current directory is not empty. Use `tola init <SITE_NAME>` to create in a subdirectory."
        );
    }

    let repo = git::create_repo(root)?;
    init_site_structure(root)?;
    init_default_config(root)?;
    init_ignored_files(
        root,
        &[config.build.output.as_path(), Path::new("/assets/images/")],
    )?;
    git::commit_all(&repo, "initial commit")?;

    Ok(())
}

/// Check if a directory is completely empty
fn is_dir_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(true);
    }
    Ok(fs::read_dir(path)?.next().is_none())
}

/// Write default configuration file
fn init_default_config(root: &Path) -> Result<()> {
    let content = toml::to_string_pretty(&SiteConfig::default())?;
    fs::write(root.join(CONFIG_FILE), content)?;
    Ok(())
}

/// Create site directory structure
fn init_site_structure(root: &Path) -> Result<()> {
    for dir in SITE_DIRS {
        let path = root.join(dir);
        if path.exists() {
            bail!(
                "Path `{}` already exists. Try `tola init <SITE_NAME>` instead.",
                path.display()
            );
        }
        fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create {}", path.display()))?;
    }
    Ok(())
}

/// Initialize .gitignore and .ignore files with specified paths
pub fn init_ignored_files(root: &Path, paths: &[&Path]) -> Result<()> {
    let content = paths
        .iter()
        .filter_map(|p| p.to_str())
        .collect::<Vec<_>>()
        .join("\n");

    for filename in IGNORE_FILES {
        let path = root.join(filename);
        if !path.exists() {
            fs::write(&path, &content)?;
        }
    }

    Ok(())
}
