//! Serve mode build functions.
//!
//! Unlike `cli::build` which uses rayon for parallel compilation,
//! serve mode uses the scheduler's priority queue to allow on-demand
//! requests (Active priority) to be processed before background
//! compilation (Background priority).

use anyhow::Result;
use rayon::prelude::*;

use crate::{
    asset, compiler, config::SiteConfig, core::ContentKind, debug, embed, freshness, generator, log, utils::git,
    utils::hooks,
    core::BuildMode,
};

/// Initialize serve build environment.
///
/// IMPORTANT: This must be called BEFORE `set_serving()` to avoid race conditions.
/// It handles:
/// 1. Clean output directory (if --clean flag)
/// 2. Initialize fonts and embedded assets
/// 3. Clear caches for accurate change detection
/// 4. Run pre hooks (CSS preprocessor etc.)
/// 5. Process all assets (sync, no priority needed)
pub fn init_serve_build(config: &SiteConfig) -> Result<()> {
    // Clean output directory BEFORE set_serving() to avoid race condition
    // where on-demand compilation writes files that get deleted
    if config.build.clean && config.build.output.exists() {
        std::fs::remove_dir_all(&config.build.output)?;
    }

    // Initialize fonts
    let font_dirs = crate::cli::build::collect_font_dirs(config);
    compiler::page::typst::init_typst(&font_dirs);

    // Create output directory with git repo
    let output_dir = config.paths().output_dir();
    if !output_dir.exists() {
        git::create_repo(&config.build.output)?;
    }

    // Write embedded assets (CSS, JS)
    embed::write_embedded_assets(config, &output_dir)?;

    // Clear caches for accurate change detection (same as init_build)
    typst_batch::clear_file_cache();
    freshness::clear_cache();

    // Run pre hooks (CSS preprocessor etc.) - IMPORTANT for Tailwind users
    hooks::run_pre_hooks(config, BuildMode::DEVELOPMENT, true)?;

    // Process all assets synchronously (no priority needed for assets)
    process_assets(config)?;

    Ok(())
}

/// Process all assets for serve mode.
fn process_assets(config: &SiteConfig) -> Result<()> {
    let clean = config.build.clean;

    // Collect asset files
    let assets: Vec<_> = config
        .build
        .assets
        .nested_sources()
        .flat_map(compiler::collect_all_files)
        .collect();

    let content_assets: Vec<_> = compiler::collect_all_files(&config.build.content)
        .into_iter()
        .filter(|p| !ContentKind::is_content_file(p))
        .collect();

    // Process in parallel
    assets.par_iter().for_each(|path| {
        let _ = asset::process_asset(path, config, clean, false);
    });

    content_assets.par_iter().for_each(|path| {
        let _ = asset::process_rel_asset(path, config, clean, false);
    });

    // Flatten assets and CNAME
    let _ = asset::process_flatten_assets(config, clean, false);
    let _ = asset::process_cname(config);

    Ok(())
}

/// Build pages using scheduler for serve mode.
///
/// Unlike `build_all()` which uses rayon, this uses the scheduler's priority queue.
/// This allows on-demand requests (Active priority) to be processed before
/// background compilation (Background priority).
pub fn serve_build(config: &SiteConfig) -> Result<()> {
    use compiler::scheduler::SCHEDULER;

    // Collect all content files
    let content_files: Vec<_> = compiler::collect_all_files(&config.build.content)
        .into_iter()
        .filter(|p| ContentKind::is_content_file(p))
        .collect();

    debug!("build"; "compiling {} pages via scheduler", content_files.len());

    // Submit all pages to scheduler with Background priority
    // On-demand requests will use Active priority and be processed first
    SCHEDULER.submit_background(content_files);

    // Wait for all background tasks to complete
    SCHEDULER.wait_all();

    // Post-processing (flatten assets already done in init_serve_build)
    // CNAME already done in init_serve_build

    // Run post hooks
    hooks::run_post_hooks(config, BuildMode::DEVELOPMENT, true)?;

    // Finalize: print warnings and persist cache
    finalize_serve_build(config)?;

    // Generate feed and sitemap
    let (rss_result, sitemap_result) = rayon::join(
        || generator::feed::build_feed(config),
        || generator::sitemap::build_sitemap(config),
    );

    rss_result?;
    sitemap_result?;

    log!("build"; "done");
    Ok(())
}

/// Finalize serve build: print warnings and persist cache.
fn finalize_serve_build(config: &SiteConfig) -> Result<()> {
    use crate::core::GLOBAL_ADDRESS_SPACE;

    // Print compiler warnings
    let warnings = compiler::drain_warnings();
    if !warnings.is_empty() {
        // Simple warning output for serve mode
        for item in warnings.iter().take(10) {
            eprintln!("{}", item);
        }
        let remaining = warnings.len().saturating_sub(10);
        if remaining > 0 {
            eprintln!("... and {} more warning(s)", remaining);
        }
    }

    // Persist VDOM cache for serve reuse
    let source_paths = GLOBAL_ADDRESS_SPACE.read().source_paths();
    if let Err(e) = crate::cache::persist_cache(
        &compiler::page::BUILD_CACHE,
        &source_paths,
        config.get_root(),
    ) {
        crate::debug!("build"; "failed to persist vdom cache: {}", e);
    }

    Ok(())
}
