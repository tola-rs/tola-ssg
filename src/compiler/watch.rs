//! Incremental build helpers for watch mode.
//!
//! This module provides the **compilation logic** for processing file changes,
//! called by the event handler in [`crate::watch`].
//!
//! # Relationship with `src/watch.rs`
//!
//! ```text
//! src/watch.rs                    compiler/watch.rs
//! ────────────────────────────    ────────────────────────────
//! • File system monitoring        • File compilation
//! • Event debouncing              • Progress display
//! • Rebuild strategy decision     • Error collection
//! • Dependency graph queries      • Tailwind rebuild
//!         │
//!         └──── calls ────────────► process_watched_files()
//! ```

use crate::compiler::assets::{process_asset, rebuild_tailwind};
use crate::compiler::pages::process_page;
use crate::config::SiteConfig;
use crate::data::virtual_fs;
use crate::logger::ProgressBars;
use crate::utils::category::{categorize_path, normalize_path, FileCategory};
use anyhow::{bail, Result};
use rayon::prelude::*;
use std::path::PathBuf;

// =============================================================================
// Public API
// =============================================================================

/// Process all watched file changes.
///
/// Categorizes files by type (content vs asset) and processes them accordingly.
/// Content files are compiled with Typst, assets are copied to output directory.
/// Shows progress bar when processing multiple files.
///
/// # Arguments
/// * `clean` - If true, skip up-to-date checks (used when dependencies changed)
///
/// Returns the number of files processed on success, or an error if any file processing fails.
pub fn process_watched_files(
    files: &[PathBuf],
    config: &'static SiteConfig,
    clean: bool,
) -> Result<usize> {
    let (content_files, asset_files) = categorize_files(files, config);

    let progress = ProgressBars::new_filtered(&[
        ("content", content_files.len()),
        ("assets", asset_files.len()),
    ]);

    // Process content files
    let content_errors = compile_content(&content_files, config, clean, progress.as_ref())?;

    // Update virtual data files on disk after content changes
    // This ensures the latest tags/pages data is available for hot-reload
    if !content_files.is_empty() {
        let _ = virtual_fs::write_to_disk(&config.build.output.join(&config.build.data));
    }

    // Process asset files
    process_assets(&asset_files, config, progress.as_ref())?;

    // Report errors (deduplicated)
    report_errors(content_errors)?;

    Ok(content_files.len() + asset_files.len())
}

// =============================================================================
// Internal Helpers
// =============================================================================

/// Categorize files into content and asset lists.
fn categorize_files<'a>(
    files: &'a [PathBuf],
    config: &SiteConfig,
) -> (Vec<&'a PathBuf>, Vec<&'a PathBuf>) {
    let mut content = Vec::new();
    let mut assets = Vec::new();

    for path in files.iter().filter(|p| p.exists()) {
        match categorize_path(path, config) {
            FileCategory::Content => content.push(path),
            FileCategory::Asset => assets.push(path),
            _ => {}
        }
    }

    (content, assets)
}

/// Compile content files, with validation-first strategy for dependency rebuilds.
///
/// When `clean` is true (dependency changed), compiles first file sequentially
/// to validate templates before parallel compilation of remaining files.
fn compile_content(
    files: &[&PathBuf],
    config: &'static SiteConfig,
    clean: bool,
    progress: Option<&ProgressBars>,
) -> Result<Vec<anyhow::Error>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let log_file = progress.is_none();

    // Validation-first: compile one file first when rebuilding dependencies
    let errors = if clean && files.len() > 1 {
        compile_with_validation(files, config, log_file, progress)?
    } else {
        compile_parallel(files, config, clean, log_file, progress)?
    };

    Ok(errors)
}

/// Compile first file to validate, then compile rest in parallel.
fn compile_with_validation(
    files: &[&PathBuf],
    config: &'static SiteConfig,
    log_file: bool,
    progress: Option<&ProgressBars>,
) -> Result<Vec<anyhow::Error>> {
    // Compile first file to validate template
    let first = normalize_path(files[0]);
    let first_result = process_page(&first, config, true, None, log_file);
    inc_progress(progress, "content");

    if let Err(e) = first_result {
        return Ok(vec![e]); // Template broken, skip rest
    }

    // Template OK, compile remaining in parallel
    let errors = compile_parallel(&files[1..], config, true, log_file, progress)?;

    Ok(errors)
}

/// Compile files in parallel, collecting errors.
fn compile_parallel(
    files: &[&PathBuf],
    config: &'static SiteConfig,
    clean: bool,
    log_file: bool,
    progress: Option<&ProgressBars>,
) -> Result<Vec<anyhow::Error>> {
    let errors: Vec<_> = files
        .par_iter()
        .filter_map(|path| {
            let path = normalize_path(path);
            let result = process_page(&path, config, clean, None, log_file);
            inc_progress(progress, "content");
            result.err()
        })
        .collect();

    // Rebuild tailwind if enabled
    if config.build.css.tailwind.enable && !files.is_empty() {
        rebuild_tailwind(config)?;
    }

    Ok(errors)
}

/// Process asset files in parallel.
fn process_assets(
    files: &[&PathBuf],
    config: &'static SiteConfig,
    progress: Option<&ProgressBars>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let log_file = progress.is_none();

    files
        .par_iter()
        .filter(|p| p.exists())
        .try_for_each(|path| {
            let path = normalize_path(path);
            let result = process_asset(&path, config, true, log_file);
            inc_progress(progress, "assets");
            result
        })
}

/// Increment progress bar by name.
#[inline]
fn inc_progress(progress: Option<&ProgressBars>, name: &str) {
    if let Some(p) = progress {
        p.inc_by_name(name);
    }
}

/// Report errors if any, deduplicating identical messages.
fn report_errors(errors: Vec<anyhow::Error>) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }

    let mut seen = rustc_hash::FxHashSet::default();
    let unique: Vec<_> = errors
        .into_iter()
        .map(|e| e.to_string())
        .filter(|e| seen.insert(e.clone()))
        .collect();

    bail!("{}", unique.join("\n"));
}
