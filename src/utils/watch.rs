//! File change processing for hot reload.
//!
//! Handles content and asset changes triggered by file watcher.

use super::build::{process_asset, process_content};
use crate::{config::SiteConfig, exec, log};
use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use std::{
    env, fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

/// Type of file change detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    /// Content file (.typ) changed
    Content,
    /// Asset file changed
    Asset,
    /// Template, utils, or config changed - requires full rebuild
    FullRebuild,
    /// Unknown file type
    Unknown,
}

/// Process changed content files (.typ)
pub fn process_watched_content(files: &[&PathBuf], config: &'static SiteConfig) -> Result<()> {
    files.par_iter().for_each(|path| {
        let path = normalize_path(path, config);
        if let Err(e) = process_content(&path, config, true, false) {
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
            let path = normalize_path(path, config);
            process_asset(&path, config, should_wait_until_stable, true)
        })
}

/// Process all watched file changes
pub fn process_watched_files(files: &[PathBuf], config: &'static SiteConfig) -> Result<()> {
    let content_files: Vec<_> = files
        .iter()
        .filter(|p| p.exists() && p.extension().is_some_and(|ext| ext == "typ"))
        .collect();

    let asset_files: Vec<_> = files
        .iter()
        .filter(|p| {
            let normalized = normalize_path(p, config);
            normalized.starts_with(&config.build.assets)
        })
        .collect();

    if !content_files.is_empty() {
        process_watched_content(&content_files, config)?;
    }
    if !asset_files.is_empty() {
        process_watched_assets(&asset_files, config, true)?;
    }

    Ok(())
}

/// Normalize path to absolute for comparison with config paths
fn normalize_path(path: &Path, _config: &SiteConfig) -> PathBuf {
    // Config paths are already absolute/canonicalized
    // Notify usually sends absolute paths, but canonicalize to be safe
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
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

    // Config paths are already absolute
    let output = config.build.output.join(relative_path);

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
