//! Site building orchestration.
//!
//! Build pipeline phases:
//! - **Pre Hooks** - User-defined pre-build commands
//! - **Init** - Typst warm-up, output repo, cache clear
//! - **Collect** - Gather content files and assets
//! - **Compile** - Parallel content compilation + asset processing
//! - **Iterative** - Rebuild iterative pages with complete metadata
//! - **Post-process** - Flatten assets, CNAME, CSS processor, enhance CSS
//! - **Post Hooks** - User-defined post-build commands
//! - **Finalize** - Cache persistence, warnings, logging

use crate::{
    asset::{process_asset, process_rel_asset},
    compiler::page::Pages,
    compiler::page::typst,
    compiler::{collect_all_files, drain_warnings},
    config::SiteConfig,
    core::{BuildMode, ContentKind, GLOBAL_ADDRESS_SPACE, is_shutdown},
    freshness::{self, ContentHash},
    hooks, log,
    logger::ProgressLine,
    package::generate_lsp_stubs,
    utils::{git, plural_count},
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

/// Collect font directories from config for font loading
pub fn collect_font_dirs(config: &SiteConfig) -> Vec<&Path> {
    let mut dirs: Vec<&Path> = vec![config.build.content.as_path()];
    dirs.extend(config.build.assets.nested_sources());
    dirs.extend(config.build.deps.iter().map(|p| p.as_path()));
    dirs
}

/// Build the entire site using two-phase compilation
///
/// Pipeline: pre-hooks -> init -> collect -> compile -> iterative -> post-process -> post-hooks -> finalize
pub fn build_site(
    mode: BuildMode,
    config: &SiteConfig,
    quiet: bool,
) -> Result<(ThreadSafeRepository, Pages)> {
    // Initialize (must be before pre hooks to clean output dir first)
    let repo = init_build(config)?;
    let deps_hash = freshness::compute_deps_hash(config);

    // Pre Hooks (after init so output dir exists and is clean)
    hooks::run_pre_hooks(config, mode, true)?;

    // Collect files
    let files = collect_build_files(config);
    let progress = create_progress(&files, quiet);

    // Compile content + process assets (parallel)
    let metadata = compile_and_process(mode, config, &files, deps_hash, progress.as_ref())?;

    // Log drafts skipped
    if !quiet && metadata.stats.has_skipped_drafts() {
        log!("build"; "{} skipped", plural_count(metadata.stats.drafts_skipped, "draft"));
    }

    // Rebuild iterative pages with complete metadata
    let pages = rebuild_iterative_pages(mode, config, deps_hash, &metadata)?;

    if let Some(p) = progress {
        p.finish();
    }

    // Post-processing
    post_process(config, quiet)?;

    // Post Hooks
    hooks::run_post_hooks(config, mode, true)?;

    // Finalize
    finalize_build(config, quiet)?;

    Ok((repo, pages))
}

/// Collected files for the build
struct BuildFiles {
    /// Asset files from nested directories
    assets: Vec<PathBuf>,
    /// Non-content files in content directory
    content_assets: Vec<PathBuf>,
    /// Content file counts by type
    typst_count: usize,
    markdown_count: usize,
}

/// Initialize build environment
fn init_build(config: &SiteConfig) -> Result<ThreadSafeRepository> {
    // Pre-warm typst library resources
    typst::init_typst(&collect_font_dirs(config));

    // Generate LSP stubs for tinymist completion
    let _ = generate_lsp_stubs(config.get_root());

    let repo = ensure_output_repo(&config.build.output, config.build.clean)?;

    if config.build.clean
        && let Err(e) = crate::cache::clear_cache_dir(config.get_root())
    {
        crate::debug!("build"; "failed to clear vdom cache: {}", e);
    }

    // Write enhance.css with config variables
    crate::embed::write_embedded_assets(config, &config.paths().output_dir())?;

    // Clear caches for accurate change detection
    typst_batch::clear_file_cache();
    freshness::clear_cache();

    Ok(repo)
}

/// Collect all files to process
fn collect_build_files(config: &SiteConfig) -> BuildFiles {
    let assets: Vec<_> = config
        .build
        .assets
        .nested_sources()
        .flat_map(collect_all_files)
        .collect();

    // Scan content directory once, then partition
    let all_content = collect_all_files(&config.build.content);
    let (content_files, content_assets): (Vec<_>, Vec<_>) = all_content
        .into_iter()
        .partition(|p| ContentKind::is_content_file(p));

    let typst_count = content_files
        .iter()
        .filter(|p| ContentKind::from_path(p) == Some(ContentKind::Typst))
        .count();

    BuildFiles {
        assets,
        content_assets,
        typst_count,
        markdown_count: content_files.len() - typst_count,
    }
}

/// Create progress display if not quiet
fn create_progress(files: &BuildFiles, quiet: bool) -> Option<ProgressLine> {
    if quiet {
        return None;
    }
    Some(ProgressLine::new(&[
        ("typst", files.typst_count),
        ("markdown", files.markdown_count),
        ("assets", files.assets.len() + files.content_assets.len()),
    ]))
}

/// Compile content and process assets in parallel
fn compile_and_process(
    mode: BuildMode,
    config: &SiteConfig,
    files: &BuildFiles,
    deps_hash: ContentHash,
    progress: Option<&ProgressLine>,
) -> Result<crate::compiler::page::MetadataResult> {
    let clean = config.build.clean;
    let has_error = AtomicBool::new(false);

    let (metadata_result, assets_result) = rayon::join(
        || {
            crate::compiler::page::build_static_pages(
                mode,
                config,
                clean,
                Some(deps_hash),
                progress,
            )
        },
        || {
            rayon::join(
                || process_assets(&files.assets, config, clean, &has_error, progress),
                || process_assets_rel(&files.content_assets, config, clean, &has_error, progress),
            )
        },
    );

    let metadata = metadata_result?;
    let (assets_res, content_assets_res) = assets_result;
    assets_res?;
    content_assets_res?;

    Ok(metadata)
}

/// Process nested asset files in parallel
fn process_assets(
    files: &[PathBuf],
    config: &SiteConfig,
    clean: bool,
    has_error: &AtomicBool,
    progress: Option<&ProgressLine>,
) -> Result<()> {
    files.par_iter().try_for_each(|path| {
        if is_shutdown() || has_error.load(Ordering::Relaxed) {
            return Err(anyhow!("Aborted"));
        }
        if let Err(e) = process_asset(path, config, clean, false) {
            if !has_error.swap(true, Ordering::Relaxed) {
                log!("error"; "{}: {:#}", path.display(), e);
            }
            return Err(anyhow!("Build failed"));
        }
        if let Some(p) = progress {
            p.inc("assets");
        }
        Ok(())
    })
}

/// Process content-relative asset files in parallel
fn process_assets_rel(
    files: &[PathBuf],
    config: &SiteConfig,
    clean: bool,
    has_error: &AtomicBool,
    progress: Option<&ProgressLine>,
) -> Result<()> {
    files.par_iter().try_for_each(|path| {
        if is_shutdown() || has_error.load(Ordering::Relaxed) {
            return Err(anyhow!("Aborted"));
        }
        if let Err(e) = process_rel_asset(path, config, clean, false) {
            if !has_error.swap(true, Ordering::Relaxed) {
                log!("error"; "{}: {:#}", path.display(), e);
            }
            return Err(anyhow!("Build failed"));
        }
        if let Some(p) = progress {
            p.inc("assets");
        }
        Ok(())
    })
}

/// Rebuild iterative pages if any exist
fn rebuild_iterative_pages(
    mode: BuildMode,
    config: &SiteConfig,
    deps_hash: ContentHash,
    metadata: &crate::compiler::page::MetadataResult,
) -> Result<Pages> {
    if !metadata.has_iterative_pages() {
        return Ok(Pages { items: vec![] });
    }

    match crate::compiler::page::rebuild_iterative_pages(
        mode,
        &metadata.iterative_paths,
        config,
        config.build.clean,
        Some(deps_hash),
        metadata.snapshot.clone(),
    ) {
        Ok(pages) => Ok(Pages { items: pages }),
        Err(e) => {
            log!("error"; "compile failed: {:#}", e);
            Err(anyhow!("Build failed"))
        }
    }
}

/// Post-processing (flatten assets, CNAME, HTML 404)
fn post_process(config: &SiteConfig, _quiet: bool) -> Result<()> {
    let clean = config.build.clean;

    // Flatten assets (files copied to output root)
    crate::asset::process_flatten_assets(config, clean, false)?;

    // Auto-generate CNAME if needed
    crate::asset::process_cname(config)?;

    // Copy HTML 404 page if configured
    copy_html_404(config)?;

    // Remove original images that are only referenced with nobg (minify mode only)
    if config.build.minify {
        crate::pipeline::transform::cleanup_nobg_originals();
    }

    Ok(())
}

/// Copy HTML 404 page to output directory if configured
fn copy_html_404(config: &SiteConfig) -> Result<()> {
    let Some(not_found) = &config.site.not_found else {
        return Ok(());
    };

    // Only handle .html files (typst files are compiled normally)
    if not_found.extension().and_then(|e| e.to_str()) != Some("html") {
        return Ok(());
    }

    let source = config.root_join(not_found);
    if !source.is_file() {
        log!("warning"; "404 page not found: {}", not_found.display());
        return Ok(());
    }

    let dest = config.build.output.join("404.html");
    fs::copy(&source, &dest).with_context(|| {
        format!(
            "Failed to copy 404 page from {} to {}",
            source.display(),
            dest.display()
        )
    })?;

    Ok(())
}
/// Finalize build (warnings, cache, logging)
fn finalize_build(config: &SiteConfig, quiet: bool) -> Result<()> {
    // Print compiler warnings with truncation
    let warnings = drain_warnings();
    if !warnings.is_empty() {
        print_warnings(&warnings, &config.build.diagnostics);
    }

    // Persist VDOM cache for serve reuse
    let source_paths = GLOBAL_ADDRESS_SPACE.read().source_paths();
    if let Err(e) = crate::cache::persist_cache(
        &crate::compiler::page::BUILD_CACHE,
        &source_paths,
        config.get_root(),
    ) {
        crate::debug!("build"; "failed to persist vdom cache: {}", e);
    }

    if !quiet {
        log_build_result(&config.build.output)?;
    }

    Ok(())
}

/// Print warnings with truncation rules applied
fn print_warnings(
    warnings: &typst_batch::Diagnostics,
    config: &crate::config::section::build::DiagnosticsConfig,
) {
    let items: Vec<_> = warnings.iter().collect();

    // Apply per-file and total limits
    let (filtered, per_file_truncated) = filter_by_file_limit(&items, config.max_warnings_per_file);
    let (filtered, total_truncated) = apply_total_limit(filtered, config.max_warnings);

    // Print with line limits
    let (printed, lines_truncated) = print_with_line_limits(&filtered, config);

    // Print truncation summary
    print_truncation_summary(
        filtered.len(),
        printed,
        per_file_truncated,
        total_truncated,
        lines_truncated,
    );
}

/// Filter warnings by per-file limit
/// Returns (filtered items, count of truncated items)
fn filter_by_file_limit<'a>(
    items: &[&'a typst_batch::DiagnosticInfo],
    max_per_file: Option<usize>,
) -> (Vec<&'a typst_batch::DiagnosticInfo>, usize) {
    use rustc_hash::FxHashMap;

    let Some(max) = max_per_file else {
        return (items.to_vec(), 0);
    };

    let mut file_counts: FxHashMap<&str, usize> = FxHashMap::default();
    let mut filtered = Vec::new();
    let mut truncated = 0;

    for item in items {
        let file = item.path.as_deref().unwrap_or("");
        let count = file_counts.entry(file).or_insert(0);

        if *count >= max {
            truncated += 1;
        } else {
            *count += 1;
            filtered.push(*item);
        }
    }

    (filtered, truncated)
}

/// Apply total warnings limit
/// Returns (filtered items, count of truncated items)
fn apply_total_limit<T>(mut items: Vec<T>, max: Option<usize>) -> (Vec<T>, usize) {
    let Some(max) = max else {
        return (items, 0);
    };

    if items.len() > max {
        let truncated = items.len() - max;
        items.truncate(max);
        (items, truncated)
    } else {
        (items, 0)
    }
}

/// Print warnings with line limits
/// Returns (count printed, whether lines were truncated)
fn print_with_line_limits(
    items: &[&typst_batch::DiagnosticInfo],
    config: &crate::config::section::build::DiagnosticsConfig,
) -> (usize, bool) {
    let mut total_lines = 0;
    let mut printed = 0;

    for item in items {
        let warning = item.to_string();

        // Apply per-warning line limit
        let output = match config.max_lines_per_warning {
            Some(max) => truncate_lines(&warning, max),
            None => warning,
        };

        let line_count = output.lines().count();

        // Check total lines limit
        if let Some(max_lines) = config.max_lines
            && total_lines + line_count > max_lines
        {
            let remaining = max_lines.saturating_sub(total_lines);
            if remaining > 0 {
                eprintln!("{}", truncate_lines(&output, remaining));
            }
            return (printed, true);
        }

        eprintln!("{output}");
        total_lines += line_count;
        printed += 1;
    }

    (printed, false)
}

/// Print summary of truncated warnings
fn print_truncation_summary(
    total_filtered: usize,
    printed: usize,
    per_file_truncated: usize,
    total_truncated: usize,
    lines_truncated: bool,
) {
    let remaining = total_filtered - printed;
    let total_hidden = per_file_truncated + total_truncated + remaining;

    if lines_truncated {
        eprintln!("  ...");
    }
    if total_hidden > 0 {
        eprintln!("... and {} more warning(s)", total_hidden);
    }
}

/// Truncate a string to max lines, appending "..." if truncated
fn truncate_lines(s: &str, max: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max {
        s.to_string()
    } else {
        let mut result: String = lines[..max].join("\n");
        result.push_str("\n  ...");
        result
    }
}

/// Ensure output directory exists with a git repository
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

fn log_build_result(output: &Path) -> Result<()> {
    let file_count = fs::read_dir(output)?
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != OsStr::new(".git"))
        .count();

    if file_count == 0 {
        log!("warn"; "output is empty, check if content has .typ or .md files");
    } else {
        log!("build"; "done");
    }

    Ok(())
}

/// Recompile modified files in parallel. Returns (path, error) for failures
pub fn recompile_files(files: &[PathBuf], mode: BuildMode) -> Vec<(String, String)> {
    use crate::compiler::page::process_page;
    use crate::config::cfg;

    let config = cfg();

    crate::debug!("recompile"; "starting parallel recompile of {} files", files.len());

    // Filter to supported content types
    let content_files: Vec<_> = files
        .iter()
        .filter(|f| ContentKind::from_path(f).is_some())
        .collect();

    // Parallel compile and collect errors
    let errors: Vec<_> = content_files
        .par_iter()
        .filter_map(|file| {
            let rel_path = file
                .strip_prefix(config.get_root())
                .unwrap_or(file)
                .display()
                .to_string();

            match process_page(mode, file, &config) {
                Ok(Some(result)) => {
                    if let Some(vdom) = result.indexed_vdom {
                        crate::compiler::page::cache_vdom(&result.permalink, vdom);
                    }
                    crate::debug!("recompile"; "ok: {}", rel_path);
                    None
                }
                Ok(None) => {
                    crate::debug!("recompile"; "skipped (draft): {}", rel_path);
                    None
                }
                Err(e) => {
                    let error_msg = format!("{}", e);
                    crate::debug!("recompile"; "error: {}: {}", rel_path, error_msg);
                    Some((rel_path, error_msg))
                }
            }
        })
        .collect();

    crate::debug!("recompile"; "finished with {} errors", errors.len());
    errors
}
