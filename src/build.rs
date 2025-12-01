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
        meta::{collect_pages, Pages},
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

    // Collect files in parallel for faster startup
    let (content_files, asset_files) = rayon::join(
        || collect_all_files(content),
        || collect_all_files(assets),
    );

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

    // Helper to process files in parallel with error handling and progress tracking
    fn process_files(
        files: &[PathBuf],
        idx: usize,
        task: impl Fn(&PathBuf) -> Result<()> + Sync,
        has_error: &AtomicBool,
        progress: &ProgressBars,
    ) -> Result<()> {
        files.par_iter().try_for_each(|path| {
            if has_error.load(Ordering::Relaxed) {
                return Err(anyhow!("Aborted"));
            }
            if let Err(e) = task(path) {
                if !has_error.swap(true, Ordering::Relaxed) {
                    let relative_path = std::env::current_dir()
                        .ok()
                        .and_then(|cwd| path.strip_prefix(cwd).ok())
                        .unwrap_or(path);
                    log!("error"; "{}: {:#}", relative_path.display(), e);
                }
                return Err(anyhow!("Build failed"));
            }
            progress.inc(idx);
            Ok(())
        })
    }

    // Process content and assets in parallel
    let clean = config.build.clean;
    let (posts_result, assets_result) = rayon::join(
        || {
            process_files(
                &content_files,
                0,
                |p| process_content(p, config, clean, deps_mtime, false),
                &has_error,
                &progress,
            )
        },
        || {
            process_files(
                &asset_files,
                1,
                |p| process_asset(p, config, false),
                &has_error,
                &progress,
            )
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
