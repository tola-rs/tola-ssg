//! Serve mode build functions.
//!
//! Unlike `cli::build` which uses rayon for parallel compilation,
//! serve mode must preserve low-latency on-demand requests while
//! warming up the rest of the site in the background.

use anyhow::Result;
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    address::SiteIndex,
    asset, compiler,
    compiler::scheduler::{CompileResult, SCHEDULER},
    config::{ConfigHandle, SiteConfig},
    core::{BuildMode, ContentKind, Priority, is_shutdown},
    debug, embed, freshness, hooks, log, seo,
};

const WARMUP_IDLE_GRACE: Duration = Duration::from_millis(1000);
const WARMUP_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct BuildWarning {
    source: PathBuf,
    message: String,
}

/// Initialize serve build environment
///
/// IMPORTANT: This must be called BEFORE `set_serving()` to avoid race conditions
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

    // Initialize fonts with nested asset mappings
    let font_dirs = crate::cli::build::collect_font_dirs(config);
    let nested_mappings = compiler::page::typst::build_nested_mappings(&config.build.assets.nested);
    compiler::page::typst::init_runtime(
        &font_dirs,
        config.get_root().to_path_buf(),
        nested_mappings,
    );

    // Ensure output directory exists
    let output_dir = config.paths().output_dir();
    if !output_dir.exists() {
        std::fs::create_dir_all(&config.build.output)?;
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

/// Process all assets for serve mode
fn process_assets(config: &SiteConfig) -> Result<()> {
    let clean = config.build.clean;

    // Collect asset files from assets directories
    let assets: Vec<_> = config
        .build
        .assets
        .nested_sources()
        .flat_map(compiler::collect_all_files)
        .collect();

    // Process in parallel
    assets.par_iter().for_each(|path| {
        let _ = asset::process_asset(path, config, clean, false);
    });

    // Flatten assets and CNAME
    let _ = asset::process_flatten_assets(config, clean, false);
    let _ = asset::process_cname(config);

    // Process content assets (non-.typ/.md files in content directory)
    let _ = asset::process_content_assets(config, clean);

    Ok(())
}

/// Build pages using scheduler for serve mode
///
/// Unlike `build_all()` which uses rayon, serve-mode warmup must not saturate
/// all cores before the first interactive request arrives.
///
/// Background warmup uses scheduler requests one page at a time so each result
/// returns warnings explicitly while still deduplicating with on-demand work
/// Warmup waits for request idle time before each page
pub fn serve_build(config: &SiteConfig, state: Arc<SiteIndex>) -> Result<()> {
    // Collect all content files
    let content_files: Vec<_> = compiler::collect_all_files(&config.build.content)
        .into_iter()
        .filter(|p| ContentKind::is_content_file(p))
        .collect();

    debug!("build"; "warming {} pages via scheduler", content_files.len());
    let mut warnings = warm_site_pages(content_files, Arc::new(config.clone()), Arc::clone(&state));

    // Recompile pages that depend on virtual packages (@tola/pages, @tola/site, etc.)
    // This ensures they have complete data after all pages are compiled
    warnings.extend(recompile_virtual_users(config, &state));

    // Post-processing (flatten assets already done in init_serve_build)
    // CNAME already done in init_serve_build

    // Run post hooks
    hooks::run_post_hooks(config, BuildMode::DEVELOPMENT, true)?;

    // Finalize: print warnings and persist cache
    finalize_serve_build(config, &state, &warnings)?;

    // Generate feed and sitemap
    let (rss_result, sitemap_result) = rayon::join(
        || state.with_pages(|pages| seo::feed::build_feed(config, pages)),
        || state.with_pages(|pages| seo::sitemap::build_sitemap(config, pages)),
    );

    rss_result?;
    sitemap_result?;

    debug!("build"; "done");
    Ok(())
}

/// Continue full-site warmup in the background after interactive serving starts.
///
/// The startup coordinator already made request-driven serving available after
/// scan completion, so this function must not block that path.
pub fn start_serve_build(config: ConfigHandle, state: Arc<SiteIndex>) {
    std::thread::spawn(move || {
        let config = config.current();
        if let Err(e) = serve_build(&config, state) {
            log!("build"; "background warmup failed: {}", e);
        }
    });
}

fn warm_site_pages(
    content_files: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    state: Arc<SiteIndex>,
) -> Vec<BuildWarning> {
    use crate::cli::serve::request_idle_for;

    let mut warnings = Vec::new();
    for path in content_files {
        // Only spend cycles on full-site warmup when the request path has been
        // quiet for a moment. This keeps startup eager work from racing page
        // loads or SPA navigation bursts.
        while !request_idle_for(WARMUP_IDLE_GRACE) {
            if is_shutdown() {
                return warnings;
            }
            std::thread::sleep(WARMUP_POLL_INTERVAL);
        }

        match SCHEDULER.compile(
            path.clone(),
            Priority::Background,
            Arc::clone(&config),
            Arc::clone(&state),
        ) {
            CompileResult::Success {
                warnings: items, ..
            } => {
                warnings.extend(items.into_iter().map(|message| BuildWarning {
                    source: path.clone(),
                    message,
                }));
            }
            CompileResult::Failed(_) | CompileResult::Skipped => {}
        }
    }
    warnings
}

/// Recompile pages that depend on virtual packages (@tola/pages, @tola/site, etc.)
///
/// This ensures iterative pages have complete data after all pages are compiled.
/// Called after initial scheduler compilation to fix race condition where
/// pages may have been compiled before page metadata was fully populated.
fn recompile_virtual_users(config: &SiteConfig, state: &SiteIndex) -> Vec<BuildWarning> {
    use crate::cli::serve::request_idle_for;
    use crate::compiler::dependency::{collect_virtual_dependents, flush_thread_local_deps};
    use crate::compiler::page::cache_vdom;
    use crate::reload::compile::{CompileOutcome, compile_page};

    let all_dependents = collect_virtual_dependents();

    if all_dependents.is_empty() {
        return Vec::new();
    }

    debug!("build"; "recompiling {} virtual package users", all_dependents.len());

    let mut warnings = Vec::new();

    // Recompile each dependent page (compile_page handles write + cache)
    for path in &all_dependents {
        while !request_idle_for(WARMUP_IDLE_GRACE) {
            if is_shutdown() {
                return warnings;
            }
            std::thread::sleep(WARMUP_POLL_INTERVAL);
        }

        let outcome = compile_page(path, config, state);
        if let CompileOutcome::Vdom {
            url_path,
            vdom,
            warnings: items,
            ..
        } = outcome
        {
            cache_vdom(&url_path, *vdom);
            warnings.extend(items.into_iter().map(|message| BuildWarning {
                source: path.clone(),
                message,
            }));
        }
    }

    // Flush dependencies recorded during recompilation
    flush_thread_local_deps();
    warnings
}

/// Finalize serve build: print warnings/errors and persist cache
fn finalize_serve_build(
    config: &SiteConfig,
    state: &SiteIndex,
    warnings: &[BuildWarning],
) -> Result<()> {
    use crate::cache::{
        PersistedDiagnostics, PersistedError, PersistedWarning, persist_cache, persist_diagnostics,
    };
    let root = config.get_root();

    // Drain compilation failures from scheduler cache
    let failures = SCHEDULER.drain_failures();
    if !failures.is_empty() {
        let max = config.build.diagnostics.max_errors.unwrap_or(usize::MAX);
        for (path, msg) in failures.iter().take(max) {
            let display_path = path.strip_prefix(root).unwrap_or(path);
            log!("error"; "{}", display_path.display());
            eprintln!("{}", msg);
        }
        let remaining = failures.len().saturating_sub(max);
        if remaining > 0 {
            eprintln!("... and {} more error(s)", remaining);
        }
    }

    // Print compiler warnings with configured limits
    if !warnings.is_empty() {
        let max = config.build.diagnostics.max_warnings.unwrap_or(usize::MAX);
        for item in warnings.iter().take(max) {
            eprintln!("{}", item.message);
        }
        let remaining = warnings.len().saturating_sub(max);
        if remaining > 0 {
            eprintln!("... and {} more warning(s)", remaining);
        }
    }

    // Persist warnings and errors for cache restore / browser replay
    let mut diagnostics = PersistedDiagnostics::new();
    for (path, msg) in failures.iter() {
        let display_path = path.strip_prefix(root).unwrap_or(path);
        let path_str = display_path.to_string_lossy().into_owned();
        diagnostics.push_error(PersistedError::new(path_str, "", msg.clone()));
    }
    for warning in warnings.iter() {
        let rel_path = warning
            .source
            .strip_prefix(root)
            .unwrap_or(&warning.source)
            .to_string_lossy()
            .into_owned();
        diagnostics.push_warning(PersistedWarning::new(rel_path, warning.message.clone()));
    }
    if let Err(e) = persist_diagnostics(&diagnostics, root) {
        crate::debug!("build"; "failed to persist diagnostics: {}", e);
    }

    // Persist VDOM cache for serve reuse
    let source_paths = state.read(|_, address| address.source_paths());
    if let Err(e) = persist_cache(
        &compiler::page::BUILD_CACHE,
        &source_paths,
        config.get_root(),
    ) {
        crate::debug!("build"; "failed to persist vdom cache: {}", e);
    }

    Ok(())
}
