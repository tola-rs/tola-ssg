//! Site building orchestration.
//!
//! Coordinates content compilation and asset processing.
//!
//! # Architecture
//!
//! ```text
//! build_site()
//!     │
//!     ├── collect_metadata()
//!     │       │
//!     │       └── Compile all pages, extract metadata → GLOBAL_SITE_DATA
//!     │           (HTML discarded - incomplete due to empty virtual JSON)
//!     │
//!     ├── compile_pages_with_data()
//!     │       │
//!     │       └── Compile all pages again → Write HTML files
//!     │           (Virtual JSON now returns complete data)
//!     │
//!     └── process_assets() ──► Copy/process asset files
//! ```

use crate::{
    compiler::{
        collect_all_files, collect_metadata, compile_pages_with_data,
        process_asset, process_rel_asset,
    },
    compiler::meta::Pages,
    config::SiteConfig,
    data::virtual_fs,
    log,
    logger::ProgressBars,
    typst_lib,
    utils::{
        category::get_deps_mtime,
        css,
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
/// Uses two-phase compilation to support virtual data files:
/// 1. Phase 1: Collect metadata from all pages (virtual JSON returns empty)
/// 2. Phase 2: Compile pages with complete data (virtual JSON returns full data)
///
/// # Arguments
/// * `config` - Site configuration
/// * `quiet` - If true, suppresses progress output (for watch mode)
///
/// Returns the collected page metadata for rss/sitemap generation.
/// If `config.build.clean` is true, clears the entire output directory first.
pub fn build_site(config: &SiteConfig, quiet: bool) -> Result<(ThreadSafeRepository, Pages)> {
    let output = &config.build.output;
    let assets = &config.build.assets;

    // Pre-warm typst library resources if using lib mode
    if config.build.typst.use_lib {
        typst_lib::warmup_with_root(config.get_root());
    }

    // Ensure output directory has git repo (for deploy)
    let repo = ensure_output_repo(output, config.build.clean)?;

    // Calculate deps mtime once for all content files
    let deps_mtime = get_deps_mtime(config);

    // Collect asset files early for progress bar
    let asset_files = collect_all_files(assets);
    let content_asset_files: Vec<_> = collect_all_files(&config.build.content)
        .into_iter()
        .filter(|p| p.extension().is_none_or(|ext| ext != "typ"))
        .collect();

    // ========================================================================
    // Collect metadata from all pages
    // ========================================================================
    // Virtual JSON files return empty data at this stage.
    // Metadata extraction is static and unaffected.
    // HTML output is discarded (incomplete due to empty JSON).

    // First, count .typ files for progress bar
    let typ_file_count = collect_all_files(&config.build.content)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "typ"))
        .count();

    if !quiet {
        log!("metadata"; "collecting...");
    }
    let metadata_progress = if quiet {
        None
    } else {
        Some(ProgressBars::new(&[("metadata", typ_file_count)]))
    };
    let page_paths = collect_metadata(config, || {
        if let Some(ref p) = metadata_progress {
            p.inc_by_name("metadata");
        }
    })?;
    if let Some(p) = metadata_progress {
        p.finish();
    }
    if !quiet {
        log!("metadata"; "found {} pages", page_paths.len());
    }

    // Create progress bars for Phase 2
    let progress = if quiet {
        None
    } else {
        Some(ProgressBars::new(&[
            ("content", page_paths.len()),
            ("assets", asset_files.len() + content_asset_files.len()),
        ]))
    };

    let has_error = AtomicBool::new(false);
    let clean = config.build.clean;

    // ========================================================================
    // Compile pages with complete data + Process assets
    // ========================================================================
    // Virtual JSON files now return complete data from GLOBAL_SITE_DATA.
    // HTML output is correct and written to disk.
    if !quiet {
        log!("compile"; "building pages...");
    }

    let (compile_result, assets_result) = rayon::join(
        || {
            // Compile all pages with complete data
            match compile_pages_with_data(&page_paths, config, clean, deps_mtime, || {
                if let Some(ref p) = progress {
                    p.inc_by_name("content");
                }
            }) {
                Ok(pages) => Ok(pages),
                Err(e) => {
                    if !has_error.swap(true, Ordering::Relaxed) {
                        log!("error"; "compile failed: {:#}", e);
                    }
                    Err(anyhow!("Build failed"))
                }
            }
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
                    if let Some(ref p) = progress {
                        p.inc_by_name("assets");
                    }
                    Ok(())
                })
            };

            // Process content assets (non-.typ files in content dir)
            let process_content_assets = || {
                content_asset_files.par_iter().try_for_each(|path| {
                    if has_error.load(Ordering::Relaxed) {
                        return Err(anyhow!("Aborted"));
                    }
                    if let Err(e) = process_rel_asset(path, config, clean, false) {
                        if !has_error.swap(true, Ordering::Relaxed) {
                            log!("error"; "{}: {:#}", path.display(), e);
                        }
                        return Err(anyhow!("Build failed"));
                    }
                    if let Some(ref p) = progress {
                        p.inc_by_name("assets");
                    }
                    Ok(())
                })
            };

            rayon::join(process_assets, process_content_assets)
        },
    );

    if let Some(p) = progress {
        p.finish();
    }

    let pages = compile_result?;
    let (assets_res, content_assets_res) = assets_result;
    assets_res?;
    content_assets_res?;

    // Write virtual data files to disk for external tools
    virtual_fs::write_to_disk(&config.build.output.join(&config.build.data))?;

    // Build Tailwind CSS if enabled
    if config.build.css.tailwind.enable {
        crate::compiler::assets::rebuild_tailwind(config, quiet)?;
    }

    // Generate auto-enhance CSS if enabled
    if config.build.css.auto_enhance {
        let enhance_output_dir = config.build.output.join(&config.build.path_prefix);
        css::cleanup_old_enhance_css(&enhance_output_dir)?;
        css::generate_enhance_css(&enhance_output_dir)?;
    }

    if !quiet {
        log_build_result(output)?;
    }

    Ok((repo, pages))
}

/// Ensure output directory exists with a git repository.
///
/// Creates the directory and repo if missing, opens existing repo otherwise.
/// When `clean` is true, removes all existing content first.
fn ensure_output_repo(output: &Path, clean: bool) -> Result<ThreadSafeRepository> {
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
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != OsStr::new(".git"))
        .count();

    if file_count == 0 {
        log!("warn"; "output is empty, check if content has .typ files");
    } else {
        log!("build"; "done");
    }

    Ok(())
}
