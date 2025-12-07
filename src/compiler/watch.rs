use crate::utils::category::{FileCategory, categorize_path, normalize_path};
use crate::compiler::pages::process_page;
use crate::compiler::assets::{process_asset, rebuild_tailwind};
use crate::config::SiteConfig;
use anyhow::{Result, bail};
use rayon::prelude::*;
use std::path::PathBuf;

/// Process all watched file changes.
///
/// Categorizes files by type (content vs asset) and processes them accordingly.
/// Content files are compiled with Typst, assets are copied to output directory.
///
/// Returns an error if any file processing fails, with details about which files failed.
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

    // Collect errors from content file processing
    let content_errors: Vec<_> = if content_files.is_empty() {
        Vec::new()
    } else {
        let errors: Vec<_> = content_files
            .par_iter()
            .filter_map(|path| {
                let path = normalize_path(path);
                process_page(&path, config, false, None, true).err()
            })
            .collect();

        // Rebuild tailwind CSS if enabled (content may have changed classes)
        if config.build.tailwind.enable {
            rebuild_tailwind(config)?;
        }

        errors
    };

    // Process asset files
    if !asset_files.is_empty() {
        asset_files
            .par_iter()
            .filter(|path| path.exists())
            .try_for_each(|path| {
                let path = normalize_path(path);
                process_asset(&path, config, true, true)
            })?;
    }

    // Report content errors if any
    if !content_errors.is_empty() {
        // Join all error messages for comprehensive reporting
        let error_msg = content_errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        bail!("{error_msg}");
    }

    Ok(())
}
