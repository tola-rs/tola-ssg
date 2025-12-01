//! Site building orchestration.
//!
//! Coordinates content compilation and asset processing.

use crate::{
    compiler::{collect_all_files, process_asset, process_content},
    config::SiteConfig,
    log,
    logger::ProgressBars,
    utils::{
        category::get_deps_mtime,
        git,
        page::{collect_pages, Pages},
    },
};
use anyhow::{Context, Result, anyhow};
use gix::ThreadSafeRepository;
use rayon::prelude::*;
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Build the entire site, processing content and assets in parallel.
///
/// Returns the collected page metadata for RSS/sitemap generation.
/// If `config.build.clean` is true, clears the entire output directory first.
pub fn build_site(config: &'static SiteConfig) -> Result<(ThreadSafeRepository, Pages)> {
    let output = &config.build.output;
    let content = &config.build.content;
    let assets = &config.build.assets;

    // Initialize output directory with git repo
    let repo = init_output_repo(output, config.build.clean)?;

    // Calculate deps mtime once for all content files
    let deps_mtime = get_deps_mtime(config);

    // Collect files first for progress tracking
    let content_files = collect_all_files(content);
    let asset_files = collect_all_files(assets);

    // Extract .typ file references for later page collection
    let typ_files: Vec<&PathBuf> = content_files
        .iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "typ"))
        .collect();

    // Create multi-line progress bars (index 0 = content, index 1 = assets)
    let progress = ProgressBars::new(&[
        ("content", content_files.len()),
        ("assets", asset_files.len()),
    ]);

    // Shared flag to abort early on error and prevent duplicate error logs
    let has_error = AtomicBool::new(false);

    // Process content and assets in parallel
    let clean = config.build.clean;
    let (posts_result, assets_result) = rayon::join(
        || {
            content_files.par_iter().try_for_each(|path| {
                if has_error.load(Ordering::Relaxed) {
                    return Err(anyhow!("Aborted"));
                }
                let result = process_content(path, config, clean, deps_mtime, false);
                if let Err(e) = result {
                    if !has_error.swap(true, Ordering::Relaxed) {
                        log!("error"; "{e}");
                    }
                    return Err(anyhow!("Build failed"));
                }
                progress.inc(0);
                Ok(())
            })
        },
        || {
            asset_files.par_iter().try_for_each(|path| {
                if has_error.load(Ordering::Relaxed) {
                    return Err(anyhow!("Aborted"));
                }
                let result = process_asset(path, config, false, false);
                if let Err(e) = result {
                    if !has_error.swap(true, Ordering::Relaxed) {
                        log!("error"; "{e}");
                    }
                    return Err(anyhow!("Build failed"));
                }
                progress.inc(1);
                Ok(())
            })
        },
    );

    progress.finish();

    posts_result?;
    assets_result?;

    // Collect page metadata for RSS/sitemap (reuse already collected .typ files)
    let pages = collect_pages(config, &typ_files);

    log_build_result(output)?;

    Ok((repo, pages))
}

/// Initialize output directory with git repository
fn init_output_repo(output: &Path, clean: bool) -> Result<ThreadSafeRepository> {
    match (output.exists(), clean) {
        (true, true) => {
            fs::remove_dir_all(output).with_context(|| {
                format!("Failed to clear output directory: {}", output.display())
            })?;
            git::create_repo(output)
        }
        (true, false) => git::open_repo(output).or_else(|_| {
            log!("git"; "initializing repo");
            git::create_repo(output)
        }),
        (false, _) => git::create_repo(output),
    }
}

/// Log build result based on output directory contents
fn log_build_result(output: &Path) -> Result<()> {
    let file_count = fs::read_dir(output)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != OsStr::new(".git"))
        .count();

    if file_count == 0 {
        log!("warn"; "output is empty, check if content has .typ files");
    } else {
        log!("build"; "done");
    }

    Ok(())
}
