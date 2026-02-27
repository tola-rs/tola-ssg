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

use crate::{config::SiteConfig, log, package::generate_lsp_stubs};
use anyhow::Result;
use std::path::Path;

pub use validate::InitMode;

/// Create a new site with default structure
///
/// # Steps
/// 1. Validate target directory
/// 2. Create directory structure
/// 3. Write configuration files
/// 4. Generate LSP stubs
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

    structure::create_structure(root)?;

    config::write_config(root)?;
    let output_dir = site_config.root_relative(&site_config.build.output);
    config::write_ignore_files(root, &output_dir)?;
    config::write_tola_template(root)?;
    config::write_tola_util(root)?;

    generate_lsp_stubs(root)?;

    log!("init"; "Site initialized successfully");
    Ok(())
}

/// Get the output directory path relative to root
///
/// Helper for external callers that need the output path
pub fn get_output_dir(config: &SiteConfig) -> &Path {
    &config.build.output
}
