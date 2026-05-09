use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    address::SiteIndex,
    asset::process_asset,
    compiler::{
        collect_all_files,
        page::{self, MetadataResult, Pages, TypstHost, WarningCollector},
    },
    config::{SiteConfig, section::build::DiagnosticsConfig},
    core::{BuildMode, ContentKind, is_shutdown},
    freshness::{self, ContentHash},
    log,
    logger::ProgressLine,
    package::generate_lsp_stubs,
};

/// Collected files for the build
pub(super) struct BuildFiles {
    /// Asset files from nested directories
    assets: Vec<PathBuf>,
    /// Content file counts by type
    typst_count: usize,
    markdown_count: usize,
}

/// Initialize build environment
pub(super) fn init_build(config: &SiteConfig) -> Result<TypstHost> {
    let typst_host = TypstHost::for_config(config);

    // Generate LSP stubs for tinymist completion
    let _ = generate_lsp_stubs(config.get_root());

    ensure_output_dir(&config.build.output, config.build.clean)?;

    if config.build.clean
        && let Err(e) = crate::cache::clear_cache_dir(config.get_root())
    {
        crate::debug!("build"; "failed to clear vdom cache: {}", e);
    }

    // Write enhance.css with config variables
    crate::embed::write_embedded_assets(config, &config.paths().output_dir())?;

    // Clear caches for accurate change detection
    freshness::clear_cache();

    Ok(typst_host)
}

/// Collect all files to process
pub(super) fn collect_build_files(config: &SiteConfig) -> BuildFiles {
    let assets: Vec<_> = config
        .build
        .assets
        .nested_sources()
        .flat_map(collect_all_files)
        .collect();

    // Count content files by type (content assets handled separately)
    let content_files = collect_all_files(&config.build.content);
    let typst_count = content_files
        .iter()
        .filter(|p| ContentKind::from_path(p) == Some(ContentKind::Typst))
        .count();
    let markdown_count = content_files
        .iter()
        .filter(|p| ContentKind::from_path(p) == Some(ContentKind::Markdown))
        .count();

    BuildFiles {
        assets,
        typst_count,
        markdown_count,
    }
}

/// Create progress display if not quiet
pub(super) fn create_progress(files: &BuildFiles, quiet: bool) -> Option<ProgressLine> {
    if quiet {
        return None;
    }
    Some(ProgressLine::new(&[
        ("typst", files.typst_count),
        ("markdown", files.markdown_count),
        ("assets", files.assets.len()),
    ]))
}

/// Compile content and process assets in parallel
pub(super) fn compile_and_process(
    mode: BuildMode,
    config: &SiteConfig,
    typst_host: &TypstHost,
    state: &SiteIndex,
    files: &BuildFiles,
    deps_hash: ContentHash,
    warnings: &WarningCollector,
    progress: Option<&ProgressLine>,
) -> Result<MetadataResult> {
    let clean = config.build.clean;
    let has_error = AtomicBool::new(false);

    let (metadata_result, assets_result) = rayon::join(
        || {
            page::build_static_pages(
                mode,
                config,
                typst_host,
                state,
                clean,
                Some(deps_hash),
                page::GlobalStateMode::Rebuild,
                warnings,
                progress,
            )
        },
        || process_assets(&files.assets, config, clean, &has_error, progress),
    );

    let metadata = metadata_result?;
    assets_result?;

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
                let display_path = path.strip_prefix(config.get_root()).unwrap_or(path);
                log!("error"; "{}: {:#}", display_path.display(), e);
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
pub(super) fn rebuild_iterative_pages(
    mode: BuildMode,
    config: &SiteConfig,
    typst_host: &TypstHost,
    state: &SiteIndex,
    deps_hash: ContentHash,
    metadata: &MetadataResult,
    warnings: &WarningCollector,
) -> Result<Pages> {
    if !metadata.has_iterative_pages() {
        return Ok(Pages { items: vec![] });
    }

    match state.with_pages(|pages| {
        page::rebuild_iterative_pages(
            mode,
            &metadata.iterative_paths,
            config,
            typst_host,
            pages,
            config.build.clean,
            Some(deps_hash),
            metadata.snapshot.clone(),
            warnings,
        )
    }) {
        Ok(pages) => Ok(Pages { items: pages }),
        Err(e) => {
            log!("error"; "compile failed: {:#}", e);
            Err(anyhow!("Build failed"))
        }
    }
}

/// Post-processing (flatten assets, CNAME, HTML 404, content assets)
pub(super) fn post_process(config: &SiteConfig, _quiet: bool) -> Result<()> {
    let clean = config.build.clean;

    // Flatten assets (files copied to output root)
    crate::asset::process_flatten_assets(config, clean, false)?;

    // Auto-generate CNAME if needed
    crate::asset::process_cname(config)?;

    // Copy content assets (non-.typ/.md files in content directory)
    crate::asset::process_content_assets(config, clean)?;

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
pub(super) fn finalize_build(
    config: &SiteConfig,
    state: &SiteIndex,
    warnings: &WarningCollector,
    quiet: bool,
) -> Result<()> {
    // Print compiler warnings with truncation
    let drained = warnings.drain();
    if !drained.is_empty() {
        print_warnings(&drained, &config.build.diagnostics, config.get_root());
    }

    // Persist VDOM cache for serve reuse
    let source_paths = state.read(|_, address| address.source_paths());
    if let Err(e) =
        crate::cache::persist_cache(&page::BUILD_CACHE, &source_paths, config.get_root())
    {
        crate::debug!("build"; "failed to persist vdom cache: {}", e);
    }

    if !quiet {
        log_build_result(&config.build.output)?;
    }

    Ok(())
}

/// Print warnings with max_warnings limit
fn print_warnings(warnings: &typst_batch::Diagnostics, config: &DiagnosticsConfig, root: &Path) {
    let max = config.max_warnings.unwrap_or(usize::MAX);
    let total = warnings.len();

    for item in warnings.iter().take(max) {
        eprintln!("{}", page::format_warning_with_prefix(item, root));
    }

    let hidden = total.saturating_sub(max);
    if hidden > 0 {
        eprintln!("... and {} more warning(s)", hidden);
    }
}

/// Ensure output directory exists and apply clean policy
fn ensure_output_dir(output: &Path, clean: bool) -> Result<()> {
    match (output.exists(), clean) {
        (true, true) => {
            fs::remove_dir_all(output).with_context(|| {
                format!("Failed to clear output directory: {}", output.display())
            })?;
            fs::create_dir_all(output)
                .with_context(|| format!("Failed to create output directory: {}", output.display()))
        }
        (true, false) => Ok(()),
        (false, _) => fs::create_dir_all(output)
            .with_context(|| format!("Failed to create output directory: {}", output.display())),
    }
}

fn log_build_result(output: &Path) -> Result<()> {
    let file_count = fs::read_dir(output)?
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != OsStr::new(".git"))
        .count();

    if file_count == 0 {
        log!("warn"; "output is empty, check if content has .typ or .md files");
    }

    Ok(())
}
