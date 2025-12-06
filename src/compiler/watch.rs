use crate::utils::category::{FileCategory, categorize_path, normalize_path};
use crate::compiler::pages::process_page;
use crate::compiler::assets::{process_asset, rebuild_tailwind};
use crate::{config::SiteConfig, log};
use anyhow::{Result, bail};
use rayon::prelude::*;
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

/// Process all watched file changes.
///
/// Categorizes files by type (content vs asset) and processes them accordingly.
/// Content files are compiled with Typst, assets are copied to output directory.
pub fn process_watched_files(files: &[PathBuf], config: &'static SiteConfig) -> Result<()> {
    let mut content_files = Vec::new();
    let mut asset_files = Vec::new();

    // Categorize files by type
    for path in files.iter().filter(|p| p.exists()) {
        match categorize_path(path, config) {
            FileCategory::Content => content_files.push(path),
            FileCategory::Asset => asset_files.push(path),
            // Dependency changes trigger full rebuild before reaching here.
            // Unknown files (e.g., .DS_Store) are silently ignored.
            _ => {}
        }
    }

    // Process content files (.typ)
    if !content_files.is_empty() {
        content_files.par_iter().for_each(|path| {
            let path = normalize_path(path);
            if let Err(e) = process_page(&path, config, false, None, true) {
                log!("watch"; "{e}");
            }
        });

        // Rebuild tailwind CSS if enabled (content may have changed classes)
        if config.build.tailwind.enable {
            rebuild_tailwind(config)?;
        }
    }

    // Process asset files
    if !asset_files.is_empty() {
        asset_files
            .par_iter()
            .filter(|path| path.exists())
            .try_for_each(|path| {
                let path = normalize_path(path);
                wait_until_stable(&path, 5)?;
                process_asset(&path, config, true, true)
            })?;
    }

    Ok(())
}

/// Wait for file to stop being written to.
fn wait_until_stable(path: &Path, max_retries: usize) -> Result<()> {
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
