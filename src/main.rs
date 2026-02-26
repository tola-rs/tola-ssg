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

use anyhow::Result;
use clap::{ColorChoice, Parser};
use cli::{Cli, Commands, build::build_site};
use config::{SiteConfig, clear_clean_flag, init_config};
use core::BuildMode;
use gix::ThreadSafeRepository;
use rustc_hash::FxHashSet;
use seo::{feed::build_feed, sitemap::build_sitemap};

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

/// Initialize build environment: fonts + embedded assets
fn init_build_env(config: &SiteConfig) -> Result<()> {
    let font_dirs = cli::build::collect_font_dirs(config);
    let nested_mappings = compiler::page::typst::build_nested_mappings(&config.build.assets.nested);
    compiler::page::typst::init_typst_with_mappings(
        &font_dirs,
        config.get_root().to_path_buf(),
        nested_mappings,
    );

    let output_dir = config.paths().output_dir();
    std::fs::create_dir_all(&output_dir)?;
    embed::write_embedded_assets(config, &output_dir)
}

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
            startup_with_cache(&config_arc);
            true
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
fn startup_with_cache(config: &SiteConfig) {
    // Initialize build environment (fonts + embedded assets)
    let _ = init_build_env(config);

    // Get files that need recompilation:
    // - from previous errors (always revalidate)
    // = with modified mtime
    let mut files_to_compile = FxHashSet::default();

    // Add error files (always revalidate errors)
    let diagnostics = cache::restore_diagnostics(config.get_root()).unwrap_or_default();
    for error in diagnostics.errors() {
        // Convert relative path to absolute
        let abs_path = config.root_join(&error.path);
        if abs_path.exists() {
            files_to_compile.insert(abs_path);
        }
    }

    // Add modified files
    let modified = cache::get_modified_files(config.get_root());
    for path in &modified.modified {
        files_to_compile.insert(path.clone());
    }

    if !files_to_compile.is_empty() {
        let files: Vec<_> = files_to_compile.into_iter().collect();
        handle_modified_files(&files, config);
    } else {
        log!("serve"; "using cached build");
    }
}

/// Recompile modified files and display/persist errors
fn handle_modified_files(files: &[std::path::PathBuf], config: &SiteConfig) {
    // log!("serve"; "recompiling {} modified file(s)", files.len());

    let errors = cli::build::recompile_files(files, BuildMode::DEVELOPMENT);

    // Persist errors for VdomActor to restore
    persist_compile_errors(&errors, config);

    // Display first error
    if let Some((path, msg)) = errors.first() {
        logger::WatchStatus::new().error(&path.to_string(), msg);
    }

    // Log summary
    if errors.is_empty() {
        log!("serve"; "using cached build (recompiled {} files)", files.len());
    } else {
        log!("serve"; "using cached build ({} error{})",
            errors.len(), if errors.len() == 1 { "" } else { "s" });
    }
}

/// Persist compile errors to diagnostics.json
fn persist_compile_errors(errors: &[(String, String)], config: &SiteConfig) {
    let mut state = cache::PersistedDiagnostics::new();
    for (path, error) in errors {
        state.push_error(cache::PersistedError::new(
            path.clone(),
            String::new(),
            error.clone(),
        ));
    }
    let _ = cache::persist_diagnostics(&state, config.get_root());
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
