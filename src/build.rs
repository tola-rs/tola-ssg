//! Site building orchestration.
//!
//! Coordinates content compilation and asset processing.
//!
//! # Architecture
//!
//! ```text
//! build_site()
//!     │
//!     ├── collect_pages()  ──► Pages (with metadata + optional pre-compiled HTML)
//!     │       │
//!     │       ├── Lib mode: compile + extract metadata (HTML cached in PageMeta)
//!     │       └── CLI mode: query metadata only (no HTML yet)
//!     │
//!     ├── compile_pages()  ──► Write HTML files
//!     │       │
//!     │       ├── Lib mode: use cached HTML from PageMeta
//!     │       └── CLI mode: compile each page
//!     │
//!     └── process_assets() ──► Copy/process asset files
//! ```

use crate::{
    compiler::{collect_all_files, collect_pages, compile_pages, process_asset, process_relative_asset},
    compiler::meta::Pages,
    config::SiteConfig,
    log,
    logger::ProgressBars,
    typst_lib,
    utils::{
        category::get_deps_mtime,
        git,
    },
};
use anyhow::{Context, Result, anyhow};
use gix::ThreadSafeRepository;
use rayon::prelude::*;
use std::{
    ffi::OsStr,
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Build the entire site, processing content and assets in parallel.
///
/// Returns the collected page metadata for rss/sitemap generation.
/// If `config.build.clean` is true, clears the entire output directory first.
pub fn build_site(config: &'static SiteConfig) -> Result<(ThreadSafeRepository, Pages)> {
    let output = &config.build.output;
    let assets = &config.build.assets;

    // Pre-warm typst library resources if using lib mode
    if config.build.typst.use_lib {
        typst_lib::warmup_with_root(config.get_root());
    }

    // Initialize output directory with git repo
    let repo = init_output_repo(output, config.build.clean)?;

    // Calculate deps mtime once for all content files
    let deps_mtime = get_deps_mtime(config);

    // Step 1: Collect pages (metadata + optional pre-compiled HTML)
    log!("collect"; "scanning content files...");
    let pages = collect_pages(config)?;
    log!("pages"; "found {} pages", pages.items.len());

    // Collect asset files
    let asset_files = collect_all_files(assets);

    // Collect non-typ content files (images, etc. in content dir)
    let content_asset_files: Vec<_> = collect_all_files(&config.build.content)
        .into_iter()
        .filter(|p| p.extension().is_none_or(|ext| ext != "typ"))
        .collect();

    // Create progress bars
    let progress = ProgressBars::new(&[
        ("content", pages.items.len()),
        ("assets", asset_files.len() + content_asset_files.len()),
    ]);

    let has_error = AtomicBool::new(false);
    let clean = config.build.clean;

    // Step 2: Compile pages and process assets in parallel
    let (compile_result, assets_result) = rayon::join(
        || {
            // Compile all pages
            if let Err(e) = compile_pages(&pages, config, clean, deps_mtime, || progress.inc(0)) {
                if !has_error.swap(true, Ordering::Relaxed) {
                    log!("error"; "compile failed: {:#}", e);
                }
                return Err(anyhow!("Build failed"));
            }
            Ok(())
        },
        || {
            // Process asset files
            let process_assets = || {
                asset_files.par_iter().try_for_each(|path| {
                    if has_error.load(Ordering::Relaxed) {
                        return Err(anyhow!("Aborted"));
                    }
                    if let Err(e) = process_asset(path, config, clean, false) {
                        if !has_error.swap(true, Ordering::Relaxed) {
                            log!("error"; "{}: {:#}", path.display(), e);
                        }
                        return Err(anyhow!("Build failed"));
                    }
                    progress.inc(1);
                    Ok(())
                })
            };

            // Process content assets (non-.typ files in content dir)
            let process_content_assets = || {
                content_asset_files.par_iter().try_for_each(|path| {
                    if has_error.load(Ordering::Relaxed) {
                        return Err(anyhow!("Aborted"));
                    }
                    if let Err(e) = process_relative_asset(path, config, clean, false) {
                        if !has_error.swap(true, Ordering::Relaxed) {
                            log!("error"; "{}: {:#}", path.display(), e);
                        }
                        return Err(anyhow!("Build failed"));
                    }
                    progress.inc(1);
                    Ok(())
                })
            };

            rayon::join(process_assets, process_content_assets)
        },
    );

    progress.finish();

    compile_result?;
    let (assets_res, content_assets_res) = assets_result;
    assets_res?;
    content_assets_res?;

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
