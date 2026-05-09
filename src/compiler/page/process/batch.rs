//! Two-phase page compilation.

use typst_batch::prelude::*;

use crate::address::{SiteIndex, conflict};
use crate::asset::{scan_content_assets, scan_flatten_assets, scan_global_assets};
use crate::compiler::dependency::{flush_thread_local_deps, record_dependencies_local};
use crate::compiler::page::write::write_page;
use crate::compiler::page::{
    BatchCompileResult, CompileStats, FileSnapshot, MetadataResult, ScannedPage, TypstBatcher,
    WarningCollector, cache_vdom, format_compile_error, scan_pages, write_redirects,
};
use crate::compiler::page::{PageCompileOutput, compile, process_typst_result};
use crate::compiler::{CompileContext, collect_all_files};
use crate::config::SiteConfig;
use crate::core::{BuildMode, ContentKind, UrlPath};
use crate::freshness::ContentHash;
use crate::logger::ProgressLine;
use crate::package::{
    build_visible_current_context_for_source, build_visible_inputs, package_sentinel,
};
use crate::page::CompiledPage;
use crate::page::{
    HashStabilityTracker, PageState, StabilityDecision, StaleLinkPolicy, StoredPageMap,
    resolve_page_link_target,
};
use crate::utils::path::slug::slugify_fragment;
use anyhow::Result;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

struct BuildContext<'a> {
    mode: BuildMode,
    config: &'a SiteConfig,
    store: &'a StoredPageMap,
    clean: bool,
    deps_hash: Option<ContentHash>,
    global_state: GlobalStateMode,
    warnings: &'a WarningCollector,
}

impl<'a> BuildContext<'a> {
    fn new(
        mode: BuildMode,
        config: &'a SiteConfig,
        store: &'a StoredPageMap,
        clean: bool,
        deps_hash: Option<ContentHash>,
        global_state: GlobalStateMode,
        warnings: &'a WarningCollector,
    ) -> Self {
        Self {
            mode,
            config,
            store,
            clean,
            deps_hash,
            global_state,
            warnings,
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

    fn rebuilds_global_state(&self) -> bool {
        self.global_state.rebuilds_global_state()
    }
}

struct BuildPageResult {
    path: PathBuf,
    page: CompiledPage,
    kind: crate::page::PageKind,
}

struct StaticBuild {
    metadata: MetadataResult,
    pages: Vec<CompiledPage>,
}

/// Global page state policy for a static page build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalStateMode {
    /// Rebuild page storage, link graph, and address space from this build.
    Rebuild,
    /// Reuse page storage, link graph, and address space already produced by scan.
    ReuseScanned,
}

impl GlobalStateMode {
    const fn rebuilds_global_state(self) -> bool {
        matches!(self, Self::Rebuild)
    }
}

/// Compile all pages. Static pages are written after conflict detection passes
///
/// Uses pre-scan optimization: always scans first to collect metadata and
/// identify iterative pages, then compiles with complete page-store data
///
/// `global_state` controls whether this build owns page storage/address-space
/// rebuilding or reuses state that a separate scan phase already populated.
pub fn build_static_pages(
    mode: BuildMode,
    config: &SiteConfig,
    state: &SiteIndex,
    clean: bool,
    deps_hash: Option<ContentHash>,
    global_state: GlobalStateMode,
    warnings: &WarningCollector,
    progress: Option<&ProgressLine>,
) -> Result<MetadataResult> {
    if global_state.rebuilds_global_state() {
        let next = SiteIndex::new();
        let build = next.with_pages(|store| {
            build_static_pages_with_store(
                mode,
                config,
                store,
                clean,
                deps_hash,
                global_state,
                warnings,
                progress,
            )
        })?;
        build_address_space(&build.pages, config, &next);
        state.replace_with(next);
        return Ok(build.metadata);
    }

    state
        .with_pages(|store| {
            build_static_pages_with_store(
                mode,
                config,
                store,
                clean,
                deps_hash,
                global_state,
                warnings,
                progress,
            )
        })
        .map(|build| build.metadata)
}

fn build_static_pages_with_store(
    mode: BuildMode,
    config: &SiteConfig,
    store: &StoredPageMap,
    clean: bool,
    deps_hash: Option<ContentHash>,
    global_state: GlobalStateMode,
    warnings: &WarningCollector,
    progress: Option<&ProgressLine>,
) -> Result<StaticBuild> {
    let ctx = BuildContext::new(
        mode,
        config,
        store,
        clean,
        deps_hash,
        global_state,
        warnings,
    );
    let content_files = collect_content_files(&config.build.content);
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    // Always pre-scan to collect metadata and identify iterative pages
    let scan_result = scan_pages(config, &typst_files, &markdown_files);
    let drafts_skipped = scan_result.drafts_skipped;

    // Report scan phase errors immediately
    scan_result.report_errors(ctx.max_errors(), ctx.config.get_root())?;

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

    // Populate page store from scan results BEFORE compilation.
    if ctx.rebuilds_global_state() {
        populate_pages(&scan_result.scanned, config, store);
    }

    // Compile Typst files
    // Always create new batcher for compile, reuse only snapshot from scan
    // This avoids duplicate warnings (scan already emitted them)
    let snapshot = scan_result.snapshot();
    let inputs = build_site_inputs(config, store)?;
    // Always compile with per-file @tola/current context to keep build
    // behavior aligned with serve and avoid scan-time under-detection when
    // current-dependent code only appears in page body.
    let batch = create_batch_with_inputs(config.get_root(), &typst_paths, snapshot, inputs)?;
    let typst_results =
        compile_typst_batch_with_context(&batch, &typst_paths, config, store, progress)?;

    let typst_processed = process_typst_files(&ctx, &typst_paths, typst_results);
    let markdown_processed = process_markdown_files(&ctx, &markdown_paths, progress);

    // Collect results - iterative pages already compiled with complete data
    let (pages, _) = collect_results(typst_processed, markdown_processed)?;

    flush_thread_local_deps();

    let url_sources = conflict::collect_url_sources(&pages, config);

    let conflicts = conflict::detect_conflicts(&url_sources, config.get_root());
    if !conflicts.is_empty() {
        let prefix = config.paths().prefix().to_string_lossy().into_owned();
        conflict::print_conflicts_with_prefix(&conflicts, &prefix);
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

    let snapshot = batch.and_then(|b: TypstBatcher| b.snapshot());
    let iterative_count = iterative_paths.len();
    let direct_count = pages.len() - iterative_count;

    Ok(StaticBuild {
        metadata: MetadataResult {
            iterative_paths,
            stats: CompileStats::new(direct_count, iterative_count, drafts_skipped),
            snapshot,
        },
        pages,
    })
}

/// Maximum iterations for metadata convergence
const MAX_ITERATIONS: usize = 5;

/// Recompile iterative pages with complete virtual data
///
/// Uses iterative compilation to handle self-referencing metadata:
/// - Compile with current page-store data
/// - Check if metadata changed (via hash)
/// - Repeat until convergence or max iterations
pub fn rebuild_iterative_pages(
    mode: BuildMode,
    paths: &[PathBuf],
    config: &SiteConfig,
    store: &StoredPageMap,
    clean: bool,
    deps_hash: Option<ContentHash>,
    snapshot: Option<FileSnapshot>,
    warnings: &WarningCollector,
) -> Result<Vec<CompiledPage>> {
    if paths.is_empty() {
        return Ok(vec![]);
    }

    let ctx = BuildContext::new(
        mode,
        config,
        store,
        clean,
        deps_hash,
        GlobalStateMode::Rebuild,
        warnings,
    );
    let (typst_paths, markdown_paths) = ContentKind::partition_by_kind(paths);

    // Iterative compilation loop
    let mut stability = HashStabilityTracker::with_oscillation_detection(store.pages_hash());
    let mut pages: Vec<CompiledPage> = Vec::new();

    for iteration in 0..MAX_ITERATIONS {
        let inputs = build_site_inputs(config, store)?;

        let batch = create_batch_compiler_with_inputs(
            config.get_root(),
            &typst_paths,
            snapshot.clone(),
            Some(inputs),
        )?;
        let typst_results =
            compile_typst_batch_with_context(&batch, &typst_paths, config, store, None)?;

        // Process results and update page store.
        let max_errors = ctx.max_errors();
        let typst_pages: Vec<Result<CompiledPage>> = typst_paths
            .par_iter()
            .zip(typst_results.into_par_iter())
            .map(|(path, result)| {
                let result = result.map_err(|e| format_compile_error(&e, max_errors))?;
                let page = CompiledPage::from_paths(path, ctx.config)?;
                let compile_ctx =
                    CompileContext::new(ctx.mode, ctx.config, ctx.store).with_route(&page.route);
                let content = process_typst_result(result, ctx.label(), &compile_ctx)?;
                process_iterative_page(&ctx, page, content)
            })
            .collect();

        // Markdown pages (currently all Direct, but kept for future support)
        let markdown_pages: Vec<Result<CompiledPage>> = markdown_paths
            .par_iter()
            .map(|path| {
                let page = CompiledPage::from_paths(path, ctx.config)?;
                let compile_ctx =
                    CompileContext::new(ctx.mode, ctx.config, ctx.store).with_route(&page.route);
                let content = compile(path, &compile_ctx)?;
                process_iterative_page(&ctx, page, content)
            })
            .collect();

        pages = typst_pages
            .into_iter()
            .chain(markdown_pages)
            .collect::<Result<Vec<_>>>()?;

        // Check convergence
        match stability.decide(store.pages_hash(), iteration, MAX_ITERATIONS) {
            StabilityDecision::Converged => {
                crate::debug!("iterative"; "converged after {} iteration(s)", iteration + 1);
                break;
            }
            StabilityDecision::Oscillating => {
                crate::log!(
                    "warn";
                    "metadata oscillating (cycle detected), stopping after {} iterations",
                    iteration + 1
                );
                break;
            }
            StabilityDecision::MaxIterationsReached => {
                crate::log!(
                    "warn";
                    "metadata did not converge after {} iterations",
                    MAX_ITERATIONS
                );
            }
            StabilityDecision::Continue => {}
        }
    }

    // Write all pages after convergence
    // Force write (clean=true) because pages() data may have changed
    let output_dir = &ctx.config.build.output;
    for page in &pages {
        write_page(page, true, ctx.deps_hash, false)?;
        write_redirects(page, output_dir)?;
    }

    Ok(pages)
}

/// Process iterative page without writing (for iteration loop)
fn process_iterative_page(
    ctx: &BuildContext,
    mut page: CompiledPage,
    result: PageCompileOutput,
) -> Result<CompiledPage> {
    let source = page.route.source.clone();
    page.apply_meta(result.meta, ctx.config);

    // Keep source->permalink mapping consistent across iterative passes.
    let state = PageState::new(ctx.store);
    state.sync_source_permalink(
        &source,
        page.route.permalink.clone(),
        StaleLinkPolicy::Clear,
    );

    // Update page store with metadata from compile phase.
    if let Some(ref meta) = page.content_meta {
        ctx.store
            .insert_page(page.route.permalink.clone(), meta.clone());
    }

    if let Some(vdom) = result.indexed_vdom {
        cache_vdom(&page.route.permalink, vdom);
    }

    ctx.warnings.collect(&result.warnings);

    page.compiled_html = Some(result.html);
    Ok(page)
}

// ============================================================================
// File Collection & Partitioning
// ============================================================================

pub fn collect_content_files(content_dir: &Path) -> Vec<PathBuf> {
    collect_all_files(content_dir)
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
fn build_site_inputs(config: &SiteConfig, store: &StoredPageMap) -> Result<typst_batch::Inputs> {
    build_visible_inputs(config, store)
}

/// Populate page metadata and link graph from pre-scan results.
pub fn populate_pages(scanned: &[ScannedPage], config: &SiteConfig, store: &StoredPageMap) {
    let state = PageState::new(store);

    // First pass: collect all page permalinks and metadata
    let mut page_permalinks: Vec<(UrlPath, &ScannedPage)> = Vec::new();

    for page in scanned {
        let Some(meta) = &page.meta else { continue };
        let Some(permalink) = store.apply_meta_for_source(&page.path, meta.clone(), config) else {
            continue;
        };
        state.insert_headings(permalink.clone(), page.headings.clone());
        page_permalinks.push((permalink, page));
    }

    // Second pass: populate resolved links.
    for (from_url, page) in &page_permalinks {
        if page.links.is_empty() {
            continue;
        }

        let targets: Vec<UrlPath> = page
            .links
            .iter()
            .filter_map(|link| resolve_page_link_target(store, from_url, &page.path, link, config))
            .collect();

        state.record_links(from_url, targets);
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
    store: &StoredPageMap,
    progress: Option<&ProgressLine>,
) -> Result<Vec<BatchCompileResult>> {
    let Some(b) = batch else { return Ok(vec![]) };
    let current_context_by_path: rustc_hash::FxHashMap<&Path, serde_json::Value> = files
        .iter()
        .map(|p| {
            let current = build_visible_current_context_for_source(config, store, p)?;
            Ok((p.as_path(), current))
        })
        .collect::<Result<_>>()?;

    b.batch_compile_with_context(files, |path| {
        if let Some(p) = progress {
            p.inc("typst");
        }
        crate::debug!("typst"; "compiled {}", path.display());
        current_context_by_path
            .get(path)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "missing precomputed @tola/current context for {}",
                    path.display()
                )
            })
    })
    .map_err(|e| anyhow::anyhow!("{}", e))
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
            let compile_ctx =
                CompileContext::new(ctx.mode, ctx.config, ctx.store).with_route(&page.route);
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
            let compile_ctx =
                CompileContext::new(ctx.mode, ctx.config, ctx.store).with_route(&page.route);
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
        if let Some(sentinel) = package_sentinel(pkg) {
            deps.push(sentinel);
        }
    }
    record_dependencies_local(&path, deps);

    // Collect warnings
    ctx.warnings.collect(&result.warnings);

    // Skip drafts
    if result.meta.as_ref().is_some_and(|m| m.draft) {
        return Ok(None);
    }

    page.apply_meta(result.meta, ctx.config); // Apply metadata/permalink FIRST
    page.compiled_html = Some(result.html);

    // Cache VDOM with the CORRECT permalink (after apply_custom_permalink)
    if let Some(vdom) = result.indexed_vdom {
        cache_vdom(&page.route.permalink, vdom);
    }

    if ctx.rebuilds_global_state() {
        let state = PageState::new(ctx.store);
        state.sync_source_permalink(&path, page.route.permalink.clone(), StaleLinkPolicy::Keep);
        ctx.store.insert_page(
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
    write_page(page, clean, deps_hash, false)?;
    write_redirects(page, output_dir)?;
    Ok(())
}

// ============================================================================
// Address Space
// ============================================================================

/// Build the address space from page metadata.
///
/// This populates the site state's address space with all pages and assets,
/// enabling internal link validation.
///
/// Uses the pure `asset::scan` module for directory traversal
pub fn build_address_space(pages: &[CompiledPage], config: &SiteConfig, state: &SiteIndex) {
    state.edit(|store, space| {
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
            let heading_ids = store
                .get_headings(&page.route.permalink)
                .into_iter()
                .map(|heading| slugify_fragment(&heading.text, &config.build.slug));
            space.register_headings(&page.route.permalink, heading_ids);
        }

        // Register global assets (nested directories)
        for asset in scan_global_assets(config) {
            space.register_asset(asset);
        }

        // Register flatten assets (individual files at output root)
        for asset in scan_flatten_assets(config) {
            space.register_asset(asset);
        }

        // Register content assets (non-.typ/.md files in content directory)
        for asset in scan_content_assets(config) {
            space.register_asset(asset);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::ResolveResult;
    use crate::compiler::page::{ScannedHeading, ScannedPage, ScannedPageLink};
    use crate::core::LinkOrigin;
    use crate::page::PageMeta;
    use std::fs;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static GLOBAL_STATE_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct GlobalStateGuard {
        state: SiteIndex,
        _lock: MutexGuard<'static, ()>,
    }

    impl GlobalStateGuard {
        fn new() -> Self {
            let lock = match GLOBAL_STATE_TEST_LOCK.lock() {
                Ok(lock) => lock,
                Err(poisoned) => poisoned.into_inner(),
            };
            let state = SiteIndex::new();
            reset_global_state(&state);
            Self { state, _lock: lock }
        }

        fn state(&self) -> &SiteIndex {
            &self.state
        }
    }

    impl Drop for GlobalStateGuard {
        fn drop(&mut self) {
            reset_global_state(&self.state);
        }
    }

    fn reset_global_state(state: &SiteIndex) {
        state.clear();
    }

    fn markdown_site(dir: &TempDir) -> SiteConfig {
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;
        config.build.output = output_dir;
        config
    }

    fn write_markdown_page(config: &SiteConfig, filename: &str, title: &str) -> PathBuf {
        let source = config.build.content.join(filename);
        fs::write(
            &source,
            format!("+++\ntitle = \"{title}\"\n+++\n\n# {title}\n"),
        )
        .unwrap();
        source
    }

    fn write_markdown_page_with_permalink(
        config: &SiteConfig,
        filename: &str,
        title: &str,
        permalink: &str,
    ) -> PathBuf {
        let source = config.build.content.join(filename);
        fs::write(
            &source,
            format!("+++\ntitle = \"{title}\"\npermalink = \"{permalink}\"\n+++\n\n# {title}\n"),
        )
        .unwrap();
        source
    }

    #[test]
    fn test_build_address_space_registers_heading_ids_for_fragment_resolution() {
        let state = GlobalStateGuard::new();
        let site = state.state();
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("post.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;
        config.build.output = output_dir;

        let page = CompiledPage::from_paths(&source, &config).unwrap();
        site.with_pages(|store| {
            store.insert_headings(
                page.route.permalink.clone(),
                vec![ScannedHeading {
                    level: 1,
                    text: "Hello World".to_string(),
                    supplement: None,
                }],
            );
        });

        build_address_space(std::slice::from_ref(&page), &config, site);

        site.read(|_, space| {
            let ctx = crate::address::ResolveContext {
                current_permalink: &page.route.permalink,
                source_path: &page.route.source,
                origin: LinkOrigin::Href,
            };

            assert!(matches!(
                space.resolve("#hello-world", &ctx),
                ResolveResult::Found(_)
            ));
            assert!(matches!(
                space.resolve("#missing", &ctx),
                ResolveResult::FragmentNotFound { .. }
            ));
        });
    }

    #[test]
    fn test_populate_pages_records_relative_page_links_only() {
        let state = GlobalStateGuard::new();
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(content_dir.join("posts")).unwrap();

        let source_a = content_dir.join("posts").join("a.md");
        let source_b = content_dir.join("posts").join("b.md");
        fs::write(&source_a, "# A").unwrap();
        fs::write(&source_b, "# B").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        let scanned = vec![
            ScannedPage {
                path: source_a.clone(),
                meta: Some(PageMeta {
                    title: Some("A".to_string()),
                    ..Default::default()
                }),
                kind: crate::page::PageKind::Direct,
                links: vec![
                    ScannedPageLink::new("../b/", LinkOrigin::Href),
                    ScannedPageLink::new("./cat.png", LinkOrigin::Src),
                ],
                headings: vec![],
            },
            ScannedPage {
                path: source_b.clone(),
                meta: Some(PageMeta {
                    title: Some("B".to_string()),
                    ..Default::default()
                }),
                kind: crate::page::PageKind::Direct,
                links: vec![],
                headings: vec![],
            },
        ];

        state.state().with_pages(|store| {
            populate_pages(&scanned, &config, store);

            let from = store
                .get_permalink_by_source(&source_a)
                .expect("source a permalink");
            let to = store
                .get_permalink_by_source(&source_b)
                .expect("source b permalink");

            let page_state = PageState::new(store);
            let links = page_state.links_to(&from);
            assert_eq!(links, vec![to.clone()]);
            assert!(page_state.linked_by(&to).contains(&from));
        });
    }

    #[test]
    fn test_build_static_pages_rebuilds_global_state_even_when_serving() {
        let state = GlobalStateGuard::new();
        let site = state.state();
        crate::core::set_serving();

        let dir = TempDir::new().unwrap();
        let config = markdown_site(&dir);
        let fresh_source = write_markdown_page(&config, "fresh.md", "Fresh");
        let warnings = WarningCollector::new();

        site.with_pages(|store| {
            store.insert_page(
                UrlPath::from_page("/stale/"),
                PageMeta {
                    title: Some("Stale".to_string()),
                    ..Default::default()
                },
            );
        });

        build_static_pages(
            BuildMode::DEVELOPMENT,
            &config,
            site,
            false,
            None,
            GlobalStateMode::Rebuild,
            &warnings,
            None,
        )
        .unwrap();

        let pages = site.with_pages(|store| store.get_pages_with_drafts());
        assert!(
            pages
                .iter()
                .any(|page| page.permalink == UrlPath::from_page("/fresh/"))
        );
        assert!(
            pages
                .iter()
                .all(|page| page.permalink != UrlPath::from_page("/stale/"))
        );
        assert_eq!(
            site.read(|_, address| address.source_for_url(&UrlPath::from_page("/fresh/"))),
            Some(crate::utils::path::normalize_path(&fresh_source))
        );
    }

    #[test]
    fn test_build_static_pages_reuse_scanned_does_not_mutate_page_state() {
        let state = GlobalStateGuard::new();
        let site = state.state();

        let dir = TempDir::new().unwrap();
        let config = markdown_site(&dir);
        let source =
            write_markdown_page_with_permalink(&config, "fresh.md", "Compiled", "/compiled/");
        let warnings = WarningCollector::new();

        site.with_pages(|store| {
            store
                .apply_meta_for_source(
                    &source,
                    PageMeta {
                        title: Some("Scanned".to_string()),
                        permalink: Some("/scanned/".to_string()),
                        ..Default::default()
                    },
                    &config,
                )
                .unwrap();
        });

        build_static_pages(
            BuildMode::DEVELOPMENT,
            &config,
            site,
            false,
            None,
            GlobalStateMode::ReuseScanned,
            &warnings,
            None,
        )
        .unwrap();

        assert_eq!(
            site.with_pages(|store| store.get_permalink_by_source(&source)),
            Some(UrlPath::from_page("/scanned/"))
        );

        let pages = site.with_pages(|store| store.get_pages_with_drafts());
        assert!(pages.iter().any(|page| {
            page.permalink == UrlPath::from_page("/scanned/")
                && page.meta.title.as_deref() == Some("Scanned")
        }));
        assert!(
            pages
                .iter()
                .all(|page| page.permalink != UrlPath::from_page("/compiled/"))
        );
    }
}
