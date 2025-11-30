//! File change processing for hot reload.
//!
//! Handles content and asset changes triggered by file watcher.

use super::build::{process_asset, process_content};
use super::category::{FileCategory, categorize_path, normalize_path};
use crate::{config::SiteConfig, exec, log};
use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

/// Process changed content files (.typ)
pub fn process_watched_content(files: &[&PathBuf], config: &'static SiteConfig) -> Result<()> {
    // In watch mode, we always force rebuild changed files (deps_mtime = None)
    // because FullRebuild is triggered separately for template/config changes
    files.par_iter().for_each(|path| {
        let path = normalize_path(path);
        if let Err(e) = process_content(&path, config, true, false, None) {
            log!("watch"; "{e}");
        }
    });

    // Rebuild tailwind CSS if enabled
    if config.build.tailwind.enable {
        rebuild_tailwind(config)?;
    }

    Ok(())
}

/// Process changed asset files
pub fn process_watched_assets(
    files: &[&PathBuf],
    config: &'static SiteConfig,
    should_wait_until_stable: bool,
) -> Result<()> {
    files
        .par_iter()
        .filter(|path| path.exists())
        .try_for_each(|path| {
            let path = normalize_path(path);
            process_asset(&path, config, should_wait_until_stable, true)
        })
}

/// Process all watched file changes
pub fn process_watched_files(files: &[PathBuf], config: &'static SiteConfig) -> Result<()> {
    let mut content_files = Vec::new();
    let mut asset_files = Vec::new();

    // Categorize files by type
    for path in files.iter().filter(|p| p.exists()) {
        match categorize_path(path, config) {
            FileCategory::Content => content_files.push(path),
            FileCategory::Asset => asset_files.push(path),
            // Dependency changes trigger full rebuild in watch.rs before reaching here.
            // Unknown files (e.g., .DS_Store) are silently ignored.
            _ => {}
        }
    }

    if !content_files.is_empty() {
        process_watched_content(&content_files, config)?;
    }
    if !asset_files.is_empty() {
        process_watched_assets(&asset_files, config, true)?;
    }

    Ok(())
}

/// Rebuild tailwind CSS
fn rebuild_tailwind(config: &'static SiteConfig) -> Result<()> {
    let input = config
        .build
        .tailwind
        .input
        .as_ref()
        .ok_or_else(|| anyhow!("Tailwind input path not configured"))?;

    let relative_path = input
        .strip_prefix(&config.build.assets)?
        .to_str()
        .ok_or_else(|| anyhow!("Invalid tailwind input path"))?;

    // Output path includes path_prefix for consistency with other assets
    let output = config
        .build
        .output
        .join(&config.build.path_prefix)
        .join(relative_path);

    exec!(
        config.get_root();
        &config.build.tailwind.command;
        "-i", input, "-o", output,
        if config.build.minify { "--minify" } else { "" }
    )?;

    Ok(())
}

/// Wait for file to stop being written to
pub fn wait_until_stable(path: &Path, max_retries: usize) -> Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    let mut last_size = fs::metadata(path)?.len();

    for _ in 0..max_retries {
        thread::sleep(POLL_INTERVAL);
        let current_size = fs::metadata(path)?.len();
        if current_size == last_size {
            return Ok(());
        }
        last_size = current_size;
    }

    bail!("File did not stabilize after {max_retries} retries")
}
