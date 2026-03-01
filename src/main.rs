//! Tola - A static site generator for Typst blogs.

#![allow(dead_code)]

mod actor;
mod address;
mod asset;
mod cache;
mod cli;
mod compiler;
mod config;
mod core;
mod embed;
mod freshness;
mod hooks;
mod image;
mod logger;
mod package;
mod page;
mod pipeline;
mod reload;
mod seo;
mod utils;

use crate::address::GLOBAL_ADDRESS_SPACE;
use crate::compiler::dependency::{self, collect_virtual_dependents};
use crate::compiler::page::{BUILD_CACHE, cache_vdom};
use crate::compiler::scheduler::SCHEDULER;
use crate::page::{PAGE_LINKS, STORED_PAGES};
use crate::reload::compile::{self, CompileOutcome};
use anyhow::Result;
use cache::{PersistedDiagnostics, PersistedError, RemovedFile};
use clap::{ColorChoice, Parser};
use cli::{Cli, Commands, build::build_site};
use config::{SiteConfig, clear_clean_flag, init_config};
use core::BuildMode;
use core::UrlPath;
use gix::ThreadSafeRepository;
use rustc_hash::{FxHashMap, FxHashSet};
use seo::{feed::build_feed, sitemap::build_sitemap};
use tola_vdom::CacheKey;

fn main() -> Result<()> {
    // Setup global Ctrl+C handler (before any blocking operations)
    core::setup_shutdown_handler()?;

    let cli: &'static Cli = Box::leak(Box::new(Cli::parse()));

    // Set global color override based on CLI option
    match cli.color {
        ColorChoice::Always => owo_colors::set_override(true),
        ColorChoice::Never => owo_colors::set_override(false),
        ColorChoice::Auto => {} // owo-colors auto-detects TTY
    }

    let config = init_config(SiteConfig::load(cli)?);

    match &cli.command {
        Commands::Init { name, dry } => cli::init::new_site(&config, name.is_some(), *dry),
        Commands::Build { .. } => build_all(&config, BuildMode::PRODUCTION).map(|_| ()),
        Commands::Deploy { .. } => {
            let repo = build_all(&config, BuildMode::PRODUCTION)?;
            cli::deploy::deploy_site(&repo, &config)
        }
        Commands::Serve { .. } => serve_with_cache(&config),
        Commands::Query { args } => cli::query::run_query(args, &config),
        Commands::Validate { .. } => cli::validate::validate_site(&config),
        Commands::Fix => cli::fix::run_fix(&config),
    }
}

// =============================================================================
// Serve Command
// =============================================================================

/// Start serve with cached build support
fn serve_with_cache(config: &SiteConfig) -> Result<()> {
    use crate::core::{set_healthy, set_serving};

    // If --clean flag is set, clear vdom cache first
    if config.build.clean
        && let Err(e) = cache::clear_cache_dir(config.get_root())
    {
        debug!("serve"; "failed to clear vdom cache: {}", e);
    }

    // Check if VDOM cache AND output dir exist - if so, we can skip initial build
    let has_cache =
        !config.build.clean && cache::has_cache(config.get_root()) && config.build.output.exists();
    debug!(
        "startup";
        "serve startup path: {}",
        if has_cache { "cache" } else { "full-build" }
    );

    // Bind HTTP server first (so we can respond with 503 during scan)
    let bound_server = cli::serve::bind_server()?;

    // Start compile scheduler workers
    compiler::scheduler::SCHEDULER.start_workers();

    // Spawn background thread for scan + build
    let config_arc = config::cfg();
    let needs_full_build = !has_cache;
    std::thread::spawn(move || {
        // Progressive serving: init → scan → set_serving → build
        // If scan fails, set_serving() so FsActor can trigger rebuild on fix.
        // If scan succeeds, delay set_serving() until initial build finishes
        // to avoid serving partially converged virtual package data.
        let scan_success = !needs_full_build || progressive_scan(&config_arc);

        if !scan_success {
            if needs_full_build {
                set_serving();
            }
            set_healthy(false);
            return;
        }

        let build_success = if needs_full_build {
            // Use scheduler-based build for priority support
            match cli::serve::serve_build(&config_arc) {
                Ok(_) => true,
                Err(e) => {
                    log!("build"; "initial build failed: {}", e);
                    false
                }
            }
        } else {
            startup_with_cache(&config_arc)
        };

        // Track whether initial build succeeded (for retry on file change)
        set_healthy(build_success);

        // Only clear clean flag after successful build
        // This ensures retry will still clean output directory
        if build_success {
            clear_clean_flag();
        }

        // Mark site as ready to serve:
        // - cache path: after startup_with_cache
        // - full-build path: after initial build completes
        if has_cache || needs_full_build {
            set_serving();
        }
    });

    bound_server.run()
}

/// Quick scan for progressive serving. Returns false if scan failed or shutdown requested
fn progressive_scan(config: &SiteConfig) -> bool {
    use crate::core::is_shutdown;

    // Initialize serve build environment (clean + assets)
    if let Err(e) = cli::serve::init_serve_build(config) {
        debug!("init"; "failed: {}", e);
        return false;
    }

    if is_shutdown() {
        return false;
    }

    if let Err(e) = cli::serve::scan_pages(config) {
        debug!("scan"; "failed: {}", e);
        return false;
    }

    if is_shutdown() {
        return false;
    }

    true
}

/// Handle startup with existing cache - detect modified files and recompile
fn startup_with_cache(config: &SiteConfig) -> bool {
    if let Err(e) = cli::serve::init_serve_build(config) {
        log!("build"; "cache startup init failed: {}", e);
        return false;
    }

    if let Err(e) = cli::serve::scan_pages(config) {
        log!("scan"; "cache startup scan failed: {}", e);
        return false;
    }

    let root = config.get_root();
    let mut diagnostics = cache::restore_diagnostics(root).unwrap_or_default();
    let mut files_to_compile = FxHashSet::default();
    let mut error_files = 0usize;

    // Always retry previous compile errors on startup.
    for error in diagnostics.errors() {
        let abs_path = crate::utils::path::normalize_path(&config.root_join(&error.path));
        if abs_path.exists() {
            files_to_compile.insert(abs_path);
            error_files += 1;
        }
    }

    let modified = cache::get_modified_files(root, &config.build.content);

    debug!(
        "startup";
        "offline changes: errors={}, created={}, removed={}, modified={}",
        error_files,
        modified.created.len(),
        modified.removed.len(),
        modified.modified.len()
    );

    cleanup_removed_files(&modified.removed, config, &mut diagnostics);

    for path in modified.created {
        files_to_compile.insert(path);
    }
    for path in modified.modified {
        files_to_compile.insert(path);
    }

    let mut compile_targets: Vec<_> = files_to_compile.into_iter().collect();
    compile_targets.sort();

    let pages_hash = STORED_PAGES.pages_hash();
    let mut stats = StartupCompileStats::default();
    if !compile_targets.is_empty() {
        stats = compile_startup_batch(
            &compile_targets,
            &modified.cached_urls_by_source,
            config,
            &mut diagnostics,
        );
    }

    // Keep @tola/pages users consistent when metadata graph changed.
    if STORED_PAGES.pages_hash() != pages_hash {
        let dependents = collect_virtual_dependents();
        if !dependents.is_empty() {
            let virtual_stats = compile_startup_batch(
                &dependents.into_iter().collect::<Vec<_>>(),
                &FxHashMap::default(),
                config,
                &mut diagnostics,
            );
            stats.success += virtual_stats.success;
            stats.failed += virtual_stats.failed;
            stats.skipped += virtual_stats.skipped;
        }
    }

    if let Err(e) = cache::persist_diagnostics(&diagnostics, root) {
        debug!("startup"; "failed to persist diagnostics: {}", e);
    }

    if let Some(first_error) = diagnostics.first_error() {
        logger::WatchStatus::new().error(&first_error.path, &first_error.error);
    }

    debug!(
        "startup";
        "compile result: success={}, failed={}, skipped={}",
        stats.success,
        stats.failed,
        stats.skipped
    );

    if stats.failed == 0 && compile_targets.is_empty() && modified.removed.is_empty() {
        log!("serve"; "using cached build");
    } else if stats.failed == 0 {
        log!("serve"; "using cached build (startup compiled {} files)", stats.success);
    } else {
        log!("serve"; "using cached build (startup compile errors: {})", stats.failed);
    }

    true
}

#[derive(Debug, Default)]
struct StartupCompileStats {
    success: usize,
    failed: usize,
    skipped: usize,
}

fn cleanup_removed_files(
    removed: &[RemovedFile],
    config: &SiteConfig,
    diagnostics: &mut PersistedDiagnostics,
) {
    if removed.is_empty() {
        return;
    }

    for item in removed {
        SCHEDULER.invalidate(&item.source_path);
        GLOBAL_ADDRESS_SPACE
            .write()
            .remove_by_source(&item.source_path);
        STORED_PAGES.remove_by_source(&item.source_path);
        dependency::remove_content(&item.source_path);
        BUILD_CACHE.remove(&CacheKey::new(item.url_path.as_str()));
        PAGE_LINKS.record(&item.url_path, vec![]);
        compile::cleanup_output_for_url(config, &item.url_path);

        let rel = item
            .source_path
            .strip_prefix(config.get_root())
            .unwrap_or(&item.source_path)
            .display()
            .to_string();
        diagnostics.clear_for(&rel);
    }
}

fn compile_startup_batch(
    paths: &[std::path::PathBuf],
    cached_urls: &FxHashMap<std::path::PathBuf, UrlPath>,
    config: &SiteConfig,
    diagnostics: &mut PersistedDiagnostics,
) -> StartupCompileStats {
    let mut stats = StartupCompileStats::default();
    let outcomes = compile::compile_startup_batch(paths, config);

    for (input_path, outcome) in paths.iter().zip(outcomes.into_iter()) {
        let rel_input = input_path
            .strip_prefix(config.get_root())
            .unwrap_or(input_path)
            .display()
            .to_string();

        match outcome {
            CompileOutcome::Vdom {
                path,
                url_path,
                vdom,
                warnings,
            } => {
                // If permalink changed since cached index, remove stale output/cache key.
                if let Some(old_url) = cached_urls.get(&path)
                    && old_url != &url_path
                {
                    BUILD_CACHE.remove(&CacheKey::new(old_url.as_str()));
                    PAGE_LINKS.record(old_url, vec![]);
                    compile::cleanup_output_for_url(config, old_url);
                }

                cache_vdom(&url_path, *vdom);

                let rel = path
                    .strip_prefix(config.get_root())
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                diagnostics.clear_errors_for(&rel);
                diagnostics.set_warnings(&rel, warnings);
                stats.success += 1;
            }
            CompileOutcome::Error {
                path,
                url_path,
                error,
            } => {
                let rel = path
                    .strip_prefix(config.get_root())
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                diagnostics.push_error(PersistedError::new(
                    rel,
                    url_path.unwrap_or_default().to_string(),
                    error,
                ));
                stats.failed += 1;
            }
            CompileOutcome::Skipped => {
                diagnostics.clear_for(&rel_input);
                stats.skipped += 1;
            }
            CompileOutcome::Reload { reason } => {
                debug!("startup"; "startup compile requested reload: {}", reason);
                diagnostics.clear_for(&rel_input);
                stats.skipped += 1;
            }
        }
    }

    stats
}

// =============================================================================
// Build Command
// =============================================================================

/// Build site and optionally generate rss/sitemap in parallel
fn build_all(config: &SiteConfig, mode: BuildMode) -> Result<ThreadSafeRepository> {
    let (repo, _pages) = build_site(mode, config, false)?;

    // Generate SEO files in parallel (feed, sitemap)
    // Note: OG tags are injected during VDOM pipeline (see HeaderInjector)
    let (rss_result, sitemap_result) = rayon::join(|| build_feed(config), || build_sitemap(config));

    rss_result?;
    sitemap_result?;
    Ok(repo)
}
