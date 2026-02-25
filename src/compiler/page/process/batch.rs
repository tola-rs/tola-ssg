//! Two-phase page compilation.

use typst_batch::prelude::*;

use crate::asset::scan_global_assets;
use crate::compiler::CompileContext;
use crate::compiler::page::write::write_page;
use crate::compiler::page::{
    BatchCompileResult, CompileStats, FileSnapshot, MetadataResult, ScannedPage, TypstBatcher,
    collect_warnings, filter_drafts, format_compile_error,
};
use crate::compiler::page::{PageCompileOutput, compile, process_typst_result};
use crate::config::SiteConfig;
use crate::core::{BuildMode, ContentKind, GLOBAL_ADDRESS_SPACE, UrlPath};
use crate::freshness::ContentHash;
use crate::page::CompiledPage;
use crate::page::{PAGE_LINKS, STORED_PAGES};
use crate::utils::path::slug::slugify_path;
use anyhow::Result;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

struct BuildContext<'a> {
    mode: BuildMode,
    config: &'a SiteConfig,
    clean: bool,
    deps_hash: Option<ContentHash>,
}

impl<'a> BuildContext<'a> {
    fn new(
        mode: BuildMode,
        config: &'a SiteConfig,
        clean: bool,
        deps_hash: Option<ContentHash>,
    ) -> Self {
        Self {
            mode,
            config,
            clean,
            deps_hash,
        }
    }

    fn label(&self) -> &str {
        &self.config.build.meta.label
    }

    fn max_errors(&self) -> usize {
        self.config
            .build
            .diagnostics
            .max_errors
            .unwrap_or(usize::MAX)
    }
}

struct BuildPageResult {
    path: PathBuf,
    page: CompiledPage,
    kind: crate::page::PageKind,
}

/// Compile all pages. Static pages are written after conflict detection passes
///
/// Uses pre-scan optimization: always scans first to collect metadata and
/// identify iterative pages, then compiles with complete STORED_PAGES data
///
/// If `is_scan_completed()` is true (progressive serving mode), skips
/// clearing/repopulating STORED_PAGES and GLOBAL_ADDRESS_SPACE since
/// scan_pages() already did this
pub fn build_static_pages(
    mode: BuildMode,
    config: &SiteConfig,
    clean: bool,
    deps_hash: Option<ContentHash>,
    progress: Option<&crate::logger::ProgressLine>,
) -> Result<MetadataResult> {
    let skip_global_state = crate::core::is_serving();

    if !skip_global_state {
        STORED_PAGES.clear();
        PAGE_LINKS.clear();
    }

    let ctx = BuildContext::new(mode, config, clean, deps_hash);
    let content_files = collect_content_files(&config.build.content);
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    // Always pre-scan to collect metadata and identify iterative pages
    let scan_result = filter_drafts(config, &typst_files, &markdown_files);
    let drafts_skipped = scan_result.drafts_skipped;

    // Report scan phase errors immediately
    scan_result.report_errors(ctx.max_errors())?;

    // Get paths and identify iterative pages from scan results
    let (scanned_typst, scanned_md) = ScannedPage::partition_by_kind(&scan_result.scanned);
    let typst_paths: Vec<&PathBuf> = scanned_typst.iter().map(|s| &s.path).collect();
    let markdown_paths: Vec<&PathBuf> = scanned_md.iter().map(|s| &s.path).collect();

    let iterative_paths: Vec<PathBuf> = scan_result
        .scanned
        .iter()
        .filter(|s| s.kind.is_iterative())
        .map(|s| s.path.clone())
        .collect();
    let has_iterative = !iterative_paths.is_empty();

    // Populate STORED_PAGES from scan results BEFORE compilation
    // Skip if scan already populated it (progressive serving mode)
    if !skip_global_state {
        populate_pages(&scan_result.scanned, config);
    }

    // Compile Typst files
    // Always create new batcher for compile, reuse only snapshot from scan
    // This avoids duplicate warnings (scan already emitted them)
    let snapshot = scan_result.batcher.as_ref().and_then(|b| b.snapshot());
    let inputs = build_site_inputs(config)?;
    let (batch, typst_results) = if has_iterative {
        // Has iterative pages: use batch_compile_with_context for per-page @tola/current
        let batch = create_batch_with_inputs(config.get_root(), &typst_paths, snapshot, inputs)?;
        let results = compile_typst_batch_with_context(&batch, &typst_paths, config, progress)?;
        (batch, results)
    } else {
        // No iterative pages: use batch_compile_each with shared inputs
        let batch = create_batch_compiler_with_inputs(
            config.get_root(),
            &typst_paths,
            snapshot,
            Some(inputs),
        )?;
        let results = compile_typst_batch(&batch, &typst_paths, progress)?;
        (batch, results)
    };

    let typst_processed = process_typst_files(&ctx, &typst_paths, typst_results);
    let markdown_processed = process_markdown_files(&ctx, &markdown_paths, progress);

    // Collect results - iterative pages already compiled with complete data
    let (pages, _) = collect_results(typst_processed, markdown_processed)?;

    crate::compiler::dependency::flush_thread_local_deps();

    let url_sources = crate::address::conflict::collect_url_sources(&pages, config);

    let conflicts = crate::address::conflict::detect_conflicts(&url_sources, config.get_root());
    if !conflicts.is_empty() {
        crate::address::conflict::print_conflicts(&conflicts);
        let total_sources: usize = conflicts.iter().map(|c| c.sources.len()).sum();
        return Err(anyhow::anyhow!(
            "build failed: {} conflicting url{}, {} source{}",
            conflicts.len(),
            crate::utils::plural_s(conflicts.len()),
            total_sources,
            crate::utils::plural_s(total_sources)
        ));
    }

    // Write non-iterative pages only (iterative pages will be written by rebuild_iterative_pages)
    write_static_pages(
        &pages,
        &iterative_paths,
        clean,
        deps_hash,
        &config.build.output,
    )?;

    // Skip rebuilding address space if scan already populated it
    if !skip_global_state {
        build_address_space(&pages, config);
    }

    let snapshot = batch.and_then(|b: TypstBatcher| b.snapshot());
    let iterative_count = iterative_paths.len();
    let direct_count = pages.len() - iterative_count;

    Ok(MetadataResult {
        iterative_paths,
        stats: CompileStats::new(direct_count, iterative_count, drafts_skipped),
        snapshot,
    })
}

/// Maximum iterations for metadata convergence
const MAX_ITERATIONS: usize = 5;

/// Recompile iterative pages with complete virtual data
///
/// Uses iterative compilation to handle self-referencing metadata:
/// - Compile with current STORED_PAGES data
/// - Check if metadata changed (via hash)
/// - Repeat until convergence or max iterations
pub fn rebuild_iterative_pages(
    mode: BuildMode,
    paths: &[PathBuf],
    config: &SiteConfig,
    clean: bool,
    deps_hash: Option<ContentHash>,
    snapshot: Option<FileSnapshot>,
) -> Result<Vec<CompiledPage>> {
    if paths.is_empty() {
        return Ok(vec![]);
    }

    let ctx = BuildContext::new(mode, config, clean, deps_hash);
    let (typst_paths, markdown_paths) = ContentKind::partition_by_kind(paths);

    // Build path -> url mapping for @tola/current injection
    // Uses permalink from STORED_PAGES (populated by populate_pages with custom permalink support)
    let path_to_url: rustc_hash::FxHashMap<&Path, UrlPath> = typst_paths
        .iter()
        .filter_map(|path| {
            STORED_PAGES
                .get_permalink_by_source(path)
                .map(|url| (path.as_path(), url))
        })
        .collect();

    // Iterative compilation loop
    let mut prev_hash = STORED_PAGES.pages_hash();
    let mut seen_hashes = rustc_hash::FxHashSet::default();
    seen_hashes.insert(prev_hash); // Record initial state for cycle detection
    let mut pages: Vec<CompiledPage> = Vec::new();

    for iteration in 0..MAX_ITERATIONS {
        let inputs = build_site_inputs(config)?;

        let batch = create_batch_compiler_with_inputs(
            config.get_root(),
            &typst_paths,
            snapshot.clone(),
            Some(inputs),
        )?;

        let typst_results = compile_typst_batch_with_closure(&batch, &typst_paths, |path| {
            path_to_url
                .get(path)
                .map(|url| STORED_PAGES.build_current_context(url))
                .unwrap_or_default()
        })?;

        // Process results (updates STORED_PAGES)
        let max_errors = ctx.max_errors();
        let typst_pages: Vec<Result<CompiledPage>> = typst_paths
            .par_iter()
            .zip(typst_results.into_par_iter())
            .map(|(path, result)| {
                let result = result.map_err(|e| format_compile_error(&e, max_errors))?;
                let page = CompiledPage::from_paths(path, ctx.config)?;
                let compile_ctx = CompileContext::new(ctx.mode, ctx.config).with_route(&page.route);
                let content = process_typst_result(result, ctx.label(), &compile_ctx)?;
                process_iterative_page(&ctx, page, content)
            })
            .collect();

        // Markdown pages (currently all Direct, but kept for future support)
        let markdown_pages: Vec<Result<CompiledPage>> = markdown_paths
            .par_iter()
            .map(|path| {
                let page = CompiledPage::from_paths(path, ctx.config)?;
                let compile_ctx = CompileContext::new(ctx.mode, ctx.config).with_route(&page.route);
                let content = compile(path, &compile_ctx)?;
                process_iterative_page(&ctx, page, content)
            })
            .collect();

        pages = typst_pages
            .into_iter()
            .chain(markdown_pages)
            .collect::<Result<Vec<_>>>()?;

        // Check convergence
        let new_hash = STORED_PAGES.pages_hash();

        if new_hash == prev_hash {
            crate::debug!("iterative"; "converged after {} iteration(s)", iteration + 1);
            break;
        }

        // Check oscillation: detect cycles of any length
        if seen_hashes.contains(&new_hash) {
            crate::log!("warn"; "metadata oscillating (cycle detected), stopping after {} iterations", iteration + 1);
            break;
        }

        if iteration == MAX_ITERATIONS - 1 {
            crate::log!("warn"; "metadata did not converge after {} iterations", MAX_ITERATIONS);
        }

        seen_hashes.insert(new_hash);
        prev_hash = new_hash;
    }

    // Write all pages after convergence
    // Force write (clean=true) because pages() data may have changed
    let output_dir = &ctx.config.build.output;
    for page in &pages {
        write_page(page, true, ctx.deps_hash, false)?;
        crate::compiler::page::write_redirects(page, output_dir)?;
    }

    Ok(pages)
}

/// Process iterative page without writing (for iteration loop)
fn process_iterative_page(
    ctx: &BuildContext,
    mut page: CompiledPage,
    result: PageCompileOutput,
) -> Result<CompiledPage> {
    page.content_meta = result.meta;
    page.apply_custom_permalink(ctx.config);

    // Update STORED_PAGES with metadata from compile phase
    if let Some(ref meta) = page.content_meta {
        STORED_PAGES.insert_page(page.route.permalink.clone(), meta.clone());
    }

    if let Some(vdom) = result.indexed_vdom {
        crate::compiler::page::cache_vdom(&page.route.permalink, vdom);
    }

    collect_warnings(&result.warnings);

    page.compiled_html = Some(result.html);
    Ok(page)
}

// ============================================================================
// File Collection & Partitioning
// ============================================================================

pub fn collect_content_files(content_dir: &Path) -> Vec<PathBuf> {
    crate::compiler::collect_all_files(content_dir)
        .into_iter()
        .filter(|p| ContentKind::from_path(p).is_some())
        .collect()
}

// ============================================================================
// Batch Compilation
// ============================================================================

fn create_batch_compiler<'a>(
    root: &'a Path,
    typst_files: &[&'a PathBuf],
) -> Result<Option<TypstBatcher<'a>>> {
    if typst_files.is_empty() {
        return Ok(None);
    }
    Compiler::new(root)
        .into_batch()
        .with_snapshot_from(typst_files)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Build inputs with site config and pages data
fn build_site_inputs(config: &SiteConfig) -> Result<typst_batch::Inputs> {
    STORED_PAGES.build_inputs(config)
}

/// Populate STORED_PAGES and PAGE_LINKS from pre-scan results
pub fn populate_pages(scanned: &[ScannedPage], config: &SiteConfig) {
    // First pass: collect all page permalinks and metadata
    let mut page_permalinks: Vec<(UrlPath, &ScannedPage)> = Vec::new();

    for page in scanned {
        let Some(meta) = &page.meta else { continue };
        let Ok(mut compiled) = CompiledPage::from_paths(&page.path, config) else {
            continue;
        };

        compiled.content_meta = Some(meta.clone());
        compiled.apply_custom_permalink(config);

        let permalink = compiled.route.permalink.clone();
        STORED_PAGES.insert_page(permalink.clone(), meta.clone());
        STORED_PAGES.insert_headings(permalink.clone(), page.headings.clone());
        STORED_PAGES.insert_source_mapping(page.path.clone(), permalink.clone());
        page_permalinks.push((permalink, page));
    }

    // Second pass: populate PAGE_LINKS with resolved links
    let slug_config = &config.build.slug;
    for (from_url, page) in &page_permalinks {
        if page.links.is_empty() {
            continue;
        }

        // Convert raw link paths to UrlPaths
        let targets: Vec<UrlPath> = page
            .links
            .iter()
            .map(|link| {
                // Extract path without fragment
                let (path, _) = crate::utils::path::route::split_path_fragment(link);
                let path = path.trim_start_matches('/');

                // Slugify and normalize to page URL
                let slugified = slugify_path(path, slug_config);
                UrlPath::from_page(&format!(
                    "/{}/",
                    slugified.to_string_lossy().trim_matches('/')
                ))
            })
            .collect();

        PAGE_LINKS.record(from_url, targets);
    }
}

/// Create batcher with inputs, optionally reusing snapshot
fn create_batch_with_inputs<'a>(
    root: &'a Path,
    paths: &[&'a PathBuf],
    snapshot: Option<FileSnapshot>,
    inputs: typst_batch::Inputs,
) -> Result<Option<TypstBatcher<'a>>> {
    if paths.is_empty() {
        return Ok(None);
    }

    let batch = Compiler::new(root).into_batch().with_inputs_obj(inputs);

    Ok(Some(if let Some(snap) = snapshot {
        batch.with_snapshot(snap)
    } else {
        batch
            .with_snapshot_from(paths)
            .map_err(|e| anyhow::anyhow!("{}", e))?
    }))
}

/// Compile with per-file context for @tola/current
fn compile_typst_batch_with_context<'a>(
    batch: &Option<TypstBatcher<'a>>,
    files: &[&PathBuf],
    config: &SiteConfig,
    progress: Option<&crate::logger::ProgressLine>,
) -> Result<Vec<BatchCompileResult>> {
    let Some(b) = batch else { return Ok(vec![]) };

    // Build path -> url mapping
    let path_to_url: rustc_hash::FxHashMap<&Path, UrlPath> = files
        .iter()
        .filter_map(|p| {
            CompiledPage::from_paths(p, config)
                .ok()
                .map(|page| (p.as_path(), page.route.permalink))
        })
        .collect();

    b.batch_compile_with_context(files, |path| {
        if let Some(p) = progress {
            p.inc("typst");
        }
        crate::debug!("typst"; "compiled {}", path.display());

        path_to_url
            .get(path)
            .map(|url| STORED_PAGES.build_current_context(url))
            .unwrap_or_default()
    })
    .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Compile with custom context closure (for rebuild_iterative_pages)
fn compile_typst_batch_with_closure<'a, F>(
    batch: &Option<TypstBatcher<'a>>,
    files: &[&PathBuf],
    context_fn: F,
) -> Result<Vec<BatchCompileResult>>
where
    F: Fn(&Path) -> serde_json::Value + Sync,
{
    match batch {
        Some(b) => b
            .batch_compile_with_context(files, context_fn)
            .map_err(|e| anyhow::anyhow!("{}", e)),
        None => Ok(vec![]),
    }
}

fn create_batch_compiler_with_inputs<'a>(
    root: &'a Path,
    typst_paths: &[&'a PathBuf],
    snapshot: Option<FileSnapshot>,
    inputs: Option<typst_batch::Inputs>,
) -> Result<Option<TypstBatcher<'a>>> {
    if typst_paths.is_empty() {
        return Ok(None);
    }
    let mut compiler = Compiler::new(root).into_batch();

    // Inject inputs if provided
    if let Some(inp) = inputs {
        compiler = compiler.with_inputs_obj(inp);
    }

    Ok(Some(if let Some(snap) = snapshot {
        compiler.with_snapshot(snap)
    } else {
        compiler
            .with_snapshot_from(typst_paths)
            .map_err(|e| anyhow::anyhow!("{}", e))?
    }))
}

#[allow(dead_code)]
fn create_batch_compiler_with_snapshot<'a>(
    root: &'a Path,
    typst_paths: &[&'a PathBuf],
    snapshot: Option<FileSnapshot>,
) -> Result<Option<TypstBatcher<'a>>> {
    create_batch_compiler_with_inputs(root, typst_paths, snapshot, None)
}

fn compile_typst_batch<'a>(
    batch: &Option<TypstBatcher<'a>>,
    files: &[&PathBuf],
    progress: Option<&crate::logger::ProgressLine>,
) -> Result<Vec<BatchCompileResult>> {
    match batch {
        Some(b) => b
            .batch_compile_each(files, |path| {
                if let Some(p) = progress {
                    p.inc("typst");
                }
                crate::debug!("typst"; "compiled {}", path.display());
            })
            .map_err(|e| anyhow::anyhow!("{}", e)),
        None => Ok(vec![]),
    }
}

// ============================================================================
// Page Processing
// ============================================================================

fn process_typst_files(
    ctx: &BuildContext,
    files: &[&PathBuf],
    results: Vec<BatchCompileResult>,
) -> Vec<Result<Option<BuildPageResult>>> {
    let max_errors = ctx.max_errors();
    files
        .par_iter()
        .zip(results.into_par_iter())
        .map(|(path, result)| {
            let result = result.map_err(|e| format_compile_error(&e, max_errors))?;
            let page = CompiledPage::from_paths(path, ctx.config)?;
            let compile_ctx = CompileContext::new(ctx.mode, ctx.config).with_route(&page.route);
            let content = process_typst_result(result, ctx.label(), &compile_ctx)?;
            finalize_static_page(ctx, page, content)
        })
        .collect()
}

fn process_markdown_files(
    ctx: &BuildContext,
    files: &[&PathBuf],
    progress: Option<&crate::logger::ProgressLine>,
) -> Vec<Result<Option<BuildPageResult>>> {
    files
        .par_iter()
        .map(|path| {
            let page = CompiledPage::from_paths(path, ctx.config)?;
            let compile_ctx = CompileContext::new(ctx.mode, ctx.config).with_route(&page.route);
            let content = compile(path, &compile_ctx)?;
            if let Some(p) = progress {
                p.inc("markdown");
            }
            finalize_static_page(ctx, page, content)
        })
        .collect()
}

// ============================================================================
// Page Finalization
// ============================================================================

/// Finalize a page during static build. Does NOT write - deferred until conflict check
fn finalize_static_page(
    ctx: &BuildContext,
    mut page: CompiledPage,
    result: PageCompileOutput,
) -> Result<Option<BuildPageResult>> {
    let path = page.route.source.clone();
    let kind = result.page_kind();

    // Record dependencies (thread-local, lock-free)
    // Include virtual package sentinels for @tola/* packages
    let mut deps = result.accessed_files;
    for pkg in &result.accessed_packages {
        if let Some(sentinel) = crate::package::package_sentinel(pkg) {
            deps.push(sentinel);
        }
    }
    crate::compiler::dependency::record_dependencies_local(&path, deps);

    // Collect warnings
    collect_warnings(&result.warnings);

    // Skip drafts
    if result.meta.as_ref().is_some_and(|m| m.draft) {
        return Ok(None);
    }

    page.content_meta = result.meta;
    page.apply_custom_permalink(ctx.config); // Apply custom permalink FIRST
    page.compiled_html = Some(result.html);

    // Update source mapping if permalink changed (e.g., permalink uses pages())
    if let Some(old_permalink) = STORED_PAGES.get_permalink_by_source(&path)
        && old_permalink != page.route.permalink
    {
        // Remove old permalink entry and update source mapping
        STORED_PAGES.remove_page(&old_permalink);
        STORED_PAGES.insert_source_mapping(path.clone(), page.route.permalink.clone());
    }

    // Cache VDOM with the CORRECT permalink (after apply_custom_permalink)
    if let Some(vdom) = result.indexed_vdom {
        crate::compiler::page::cache_vdom(&page.route.permalink, vdom);
    }

    // Store in global data (skip if scan already populated it)
    if !crate::core::is_serving() {
        STORED_PAGES.insert_page(
            page.route.permalink.clone(),
            page.content_meta.clone().unwrap_or_default(),
        );
    }

    // NOTE: Writing is deferred until after conflict detection

    Ok(Some(BuildPageResult { path, page, kind }))
}

// ============================================================================
// Result Collection
// ============================================================================

fn collect_results(
    typst: Vec<Result<Option<BuildPageResult>>>,
    markdown: Vec<Result<Option<BuildPageResult>>>,
) -> Result<(Vec<CompiledPage>, Vec<PathBuf>)> {
    let mut pages = Vec::new();
    let mut iterative_paths = Vec::new();

    for result in typst.into_iter().chain(markdown) {
        if let Some(pr) = result? {
            if pr.kind.is_iterative() {
                iterative_paths.push(pr.path);
            }
            pages.push(pr.page);
        }
    }

    Ok((pages, iterative_paths))
}

// ============================================================================
// Page Writing
// ============================================================================

/// Write all static pages to disk
///
/// This is called after conflict detection passes. It writes all non-iterative
/// pages and generates redirect HTML for aliases
fn write_static_pages(
    pages: &[CompiledPage],
    iterative_paths: &[PathBuf],
    clean: bool,
    deps_hash: Option<ContentHash>,
    output_dir: &Path,
) -> Result<()> {
    // Filter to get only direct pages
    let direct_pages = filter_direct_pages(pages, iterative_paths);

    // Write all direct pages in parallel
    direct_pages
        .par_iter()
        .try_for_each(|page| write_single_page(page, clean, deps_hash, output_dir))
}

/// Filter pages to exclude iterative ones
fn filter_direct_pages<'a>(
    pages: &'a [CompiledPage],
    iterative_paths: &[PathBuf],
) -> Vec<&'a CompiledPage> {
    use rustc_hash::FxHashSet;

    let iterative_set: FxHashSet<&Path> = iterative_paths.iter().map(|p| p.as_path()).collect();
    pages
        .iter()
        .filter(|page| !iterative_set.contains(page.route.source.as_path()))
        .collect()
}

/// Write a single page: HTML file and redirects
fn write_single_page(
    page: &CompiledPage,
    clean: bool,
    deps_hash: Option<ContentHash>,
    output_dir: &Path,
) -> Result<()> {
    use crate::compiler::page::write_redirects;

    write_page(page, clean, deps_hash, false)?;
    write_redirects(page, output_dir)?;
    Ok(())
}

// ============================================================================
// Address Space
// ============================================================================

/// Build the global address space from page metadata
///
/// This populates `GLOBAL_ADDRESS_SPACE` with all pages and assets,
/// enabling internal link validation
///
/// Uses the pure `asset::scan` module for directory traversal
pub fn build_address_space(pages: &[CompiledPage], config: &SiteConfig) {
    let mut space = GLOBAL_ADDRESS_SPACE.write();
    space.clear();

    // Use primary nested entry's output name as assets prefix
    let assets_prefix = config
        .build
        .assets
        .nested
        .first()
        .map(|e| e.output_name())
        .unwrap_or("assets");
    space.set_assets_prefix(assets_prefix);
    space.set_slug_config(config.build.slug.clone());

    // Register pages
    for page in pages {
        let title = page.content_meta.as_ref().and_then(|m| m.title.clone());
        space.register_page(page.route.clone(), title);
    }

    // Register global assets (nested directories)
    for asset in scan_global_assets(config) {
        space.register_asset(asset);
    }

    // Register flatten assets (individual files at output root)
    for asset in crate::asset::scan_flatten_assets(config) {
        space.register_asset(asset);
    }

    // Register content assets (non-.typ/.md files in content directory)
    for asset in crate::asset::scan_content_assets(config) {
        space.register_asset(asset);
    }
}
