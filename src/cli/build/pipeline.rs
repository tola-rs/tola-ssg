use anyhow::{Context, Result, anyhow};
use std::{ffi::OsStr, fs, path::Path};

use crate::{
    address::SiteIndex,
    compiler::{
        collect_all_files,
        page::{self, MetadataResult, Pages, TypstHost, WarningCollector},
    },
    config::{SiteConfig, section::build::DiagnosticsConfig},
    core::{BuildMode, ContentKind, is_shutdown},
    freshness::{self, ContentHash},
    logger,
    package::generate_lsp_stubs,
};

/// Collected files for the build
pub(super) struct BuildFiles {
    /// Asset routes found by the asset scanners.
    asset_count: usize,
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
        logger::debug("build", format_args!("failed to clear vdom cache: {}", e));
    }

    // Write enhance.css with config variables
    crate::embed::write_embedded_assets(config, &config.paths().output_dir())?;

    // Clear caches for accurate change detection
    freshness::clear_cache();
    crate::asset::version::clear();

    Ok(typst_host)
}

/// Collect all files to process
pub(super) fn collect_build_files(config: &SiteConfig) -> BuildFiles {
    let asset_count = crate::asset::scan_nested_assets(config).len()
        + crate::asset::scan_flatten_assets(config).len()
        + crate::asset::scan_content_assets(config).len();

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
        asset_count,
        typst_count,
        markdown_count,
    }
}

/// Create progress display if not quiet
pub(super) fn create_progress(files: &BuildFiles, quiet: bool) -> Option<logger::ProgressLine> {
    if quiet {
        return None;
    }
    Some(logger::ProgressLine::new(&[
        ("typst", files.typst_count),
        ("markdown", files.markdown_count),
        ("assets", files.asset_count),
    ]))
}

/// Process assets, then compile content.
pub(super) fn compile_and_process(
    mode: BuildMode,
    config: &SiteConfig,
    typst_host: &TypstHost,
    state: &SiteIndex,
    deps_hash: ContentHash,
    warnings: &WarningCollector,
    progress: Option<&logger::ProgressLine>,
) -> Result<MetadataResult> {
    process_assets(config, progress)?;

    page::build_static_pages(page::StaticPageBuild {
        mode,
        config,
        typst_host,
        state,
        clean: config.build.clean,
        deps_hash: Some(deps_hash),
        global_state: page::GlobalStateMode::Rebuild,
        warnings,
        progress,
    })
}

/// Process configured asset files through the unified asset routing rules.
fn process_assets(config: &SiteConfig, progress: Option<&logger::ProgressLine>) -> Result<()> {
    if is_shutdown() {
        return Err(anyhow!("Aborted"));
    }

    let summary = crate::asset::process_configured_assets(config, false, false).map_err(|e| {
        logger::log("error", format_args!("asset processing failed: {:#}", e));
        anyhow!("Build failed")
    })?;

    if let Some(progress) = progress {
        for _ in 0..summary.scanned {
            progress.inc("assets");
        }
    }

    Ok(())
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
        page::rebuild_iterative_pages(page::IterativePageBuild {
            mode,
            paths: &metadata.iterative_paths,
            config,
            typst_host,
            store: pages,
            clean: config.build.clean,
            deps_hash: Some(deps_hash),
            snapshot: metadata.snapshot.clone(),
            warnings,
        })
    }) {
        Ok(pages) => Ok(Pages { items: pages }),
        Err(e) => {
            logger::log("error", format_args!("compile failed: {:#}", e));
            Err(anyhow!("Build failed"))
        }
    }
}

/// Post-processing (CNAME, HTML 404, cleanup)
pub(super) fn post_process(config: &SiteConfig, _quiet: bool) -> Result<()> {
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
        logger::log(
            "warning",
            format_args!("404 page not found: {}", not_found.display()),
        );
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
        logger::debug("build", format_args!("failed to persist vdom cache: {}", e));
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
        logger::text(&page::format_warning_with_prefix(item, root));
    }

    let hidden = total.saturating_sub(max);
    if hidden > 0 {
        logger::text(&format!("... and {} more warning(s)", hidden));
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
        logger::log(
            "warn",
            format_args!("output is empty, check if content has page files"),
        );
    }

    Ok(())
}
