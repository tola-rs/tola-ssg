//! Site initialization module.
//!
//! Creates new site structure with default configuration.
//!
//! # Module Structure
//!
//! - [`validate`]: Pre-initialization validation
//! - [`structure`]: Directory structure creation
//! - [`config`]: Configuration file generation

mod config;
mod structure;
mod validate;

use crate::{config::SiteConfig, log, package::generate_lsp_stubs, utils::git};
use anyhow::Result;
use std::path::Path;

pub use validate::InitMode;

/// Create a new site with default structure
///
/// # Steps
/// 1. Validate target directory
/// 2. Initialize git repository
/// 3. Create directory structure
/// 4. Write configuration files
/// 5. Generate LSP stubs
/// 6. Create initial commit
///
/// If `dry_run` is true, only prints the config template to stdout
pub fn new_site(site_config: &SiteConfig, has_name: bool, dry_run: bool) -> Result<()> {
    if dry_run {
        print!("{}", config::generate_config_template());
        return Ok(());
    }

    let root = site_config.get_root();
    let mode = if has_name {
        InitMode::NewDir
    } else {
        InitMode::CurrentDir
    };

    if let Err(e) = validate::validate_target(root, mode) {
        log!("error"; "{}", e);
        std::process::exit(1);
    }

    let repo = match git::create_repo(root) {
        Ok(repo) => repo,
        Err(e) => {
            log!("error"; "Failed to initialize git repository: {}", e);
            std::process::exit(1);
        }
    };

    structure::create_structure(root)?;

    config::write_config(root)?;
    let output_dir = site_config.root_relative(&site_config.build.output);
    config::write_ignore_files(root, &output_dir)?;
    config::write_tola_template(root)?;

    generate_lsp_stubs(root)?;

    let _ = git::commit_all(&repo, "initial commit")?;

    log!("init"; "Site initialized successfully");
    Ok(())
}

/// Get the output directory path relative to root
///
/// Helper for external callers that need the output path
pub fn get_output_dir(config: &SiteConfig) -> &Path {
    &config.build.output
}
