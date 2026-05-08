//! Common utilities shared across CLI commands.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossbeam::queue::SegQueue;
use serde_json::Value as JsonValue;
use typst_batch::prelude::*;

use crate::compiler::CompileContext;
use crate::compiler::collect_all_files;
use crate::compiler::family::Indexed;
use crate::compiler::page::scan;
use crate::compiler::page::typst::{MAX_METADATA_SCAN_ITERATIONS, scan_single_with_current};
use crate::config::SiteConfig;
use crate::core::{BuildMode, ContentKind};
use crate::package::build_visible_inputs;
use crate::page::{HashStabilityTracker, PageKind, PageMeta, StabilityDecision, StoredPageMap};
use crate::utils::path::resolve_path;
use tola_vdom::Document;

/// Lock-free parallel result collector using `SegQueue`
pub struct ParallelCollector<T> {
    queue: SegQueue<T>,
}

impl<T> ParallelCollector<T> {
    /// Create a new empty collector.
    #[inline]
    pub fn new() -> Self {
        Self {
            queue: SegQueue::new(),
        }
    }

    /// Push an item (lock-free, wait-free).
    #[inline]
    pub fn push(&self, item: T) {
        self.queue.push(item);
    }

    /// Drain all items into a Vec.
    #[allow(dead_code)]
    pub fn drain(self) -> Vec<T> {
        let mut results = Vec::new();
        while let Some(item) = self.queue.pop() {
            results.push(item);
        }
        results
    }

    /// Drain all items with pre-allocated capacity.
    pub fn drain_with_capacity(self, capacity: usize) -> Vec<T> {
        let mut results = Vec::with_capacity(capacity);
        while let Some(item) = self.queue.pop() {
            results.push(item);
        }
        results
    }
}

impl<T> Default for ParallelCollector<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Collect content files based on CLI paths
pub fn collect_content_files(paths: &[PathBuf], content_dir: &Path) -> Result<Vec<PathBuf>> {
    // Handle stdin case: read paths from stdin when `-` is passed
    let paths: Vec<PathBuf> = if paths.len() == 1 && paths[0].as_os_str() == "-" {
        read_paths_from_stdin()?
    } else {
        paths.to_vec()
    };

    if paths.is_empty() {
        // No paths specified: collect all content files
        let all_files = collect_all_files(content_dir);
        return Ok(filter_content_files(all_files));
    }

    // Collect files from all specified paths
    let mut all_files = Vec::new();
    for path in &paths {
        let resolved = resolve_path(path, content_dir);

        if resolved.is_file() {
            if ContentKind::from_path(&resolved).is_some() {
                all_files.push(resolved);
            } else {
                anyhow::bail!("Not a supported content file: {}", path.display());
            }
        } else if resolved.is_dir() {
            let dir_files = collect_all_files(&resolved);
            all_files.extend(filter_content_files(dir_files));
        } else {
            // Provide helpful error message
            let content_relative = content_dir.join(path);
            anyhow::bail!(
                "Path not found: {}\n  Tried:\n    - {}\n    - {}",
                path.display(),
                path.display(),
                content_relative.display()
            );
        }
    }

    Ok(all_files)
}

/// Read file paths from stdin, one per line
pub fn read_paths_from_stdin() -> Result<Vec<PathBuf>> {
    let stdin = io::stdin();
    let mut paths = Vec::new();

    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }

    Ok(paths)
}

/// Filter a list of paths to only include supported content files
pub fn filter_content_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|p| ContentKind::from_path(p).is_some())
        .collect()
}

/// Calculate URL path from file path relative to content directory
pub fn calculate_url_path(file: &Path, content_dir: &Path) -> String {
    let rel = file.strip_prefix(content_dir).unwrap_or(file);
    let stem = rel.with_extension("");

    let mut url = String::from("/");
    for component in stem.components() {
        if let std::path::Component::Normal(s) = component {
            let s = s.to_string_lossy();
            if s != "index" {
                url.push_str(&s);
                url.push('/');
            }
        }
    }

    if url.len() > 1 && !url.ends_with('/') {
        url.push('/');
    }

    url
}

/// Result of scanning a Markdown file via VDOM pipeline
pub struct MarkdownScanResult {
    /// Indexed VDOM for link/asset extraction.
    pub indexed_vdom: Document<Indexed>,
    /// Raw metadata JSON (if any).
    pub raw_meta: Option<JsonValue>,
}

/// Scan a Markdown file using the VDOM pipeline
pub fn scan_markdown_file(
    file: &Path,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<MarkdownScanResult> {
    let ctx = CompileContext::new(BuildMode::PRODUCTION, config, store);
    let result = scan(file, &ctx)?;

    Ok(MarkdownScanResult {
        indexed_vdom: result.indexed_vdom,
        raw_meta: result.raw_meta,
    })
}

/// Batch scan Typst files using `Batcher::for_scan()`
///
/// Does NOT set Phase - defaults to `filter`, so `pages()` in content body
/// returns empty array silently. Used for link extraction in validate
pub fn batch_scan_typst(files: &[&PathBuf], root: &Path) -> Vec<Option<typst_batch::ScanResult>> {
    if files.is_empty() {
        return vec![];
    }

    // Inject format="html" so image show rules can detect HTML output during scan.
    //
    // Scan phase is Eval-only (no Layout), so `context { target() }` won't work.
    // Image show rules use `is-html` (sys.inputs.format) to output <img> tags,
    // which allows LinkExtractor to find image src paths for validation.
    let scanner = match Batcher::for_scan(root)
        .with_inputs([("format", "html")])
        .with_snapshot_from(files)
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            return files.iter().map(|_| None).collect();
        }
    };

    match scanner.batch_scan(files) {
        Ok(results) => results
            .into_iter()
            .zip(files)
            .map(|(result, _file)| match result {
                Ok(scan) => Some(scan),
                Err(e) => {
                    eprintln!("{}", e);
                    None
                }
            })
            .collect(),
        Err(e) => {
            eprintln!("{}", e);
            files.iter().map(|_| None).collect()
        }
    }
}

/// Batch scan Typst files for metadata, failing on first error
///
/// Simple scan without iterative support. Use for validate or when
/// The page store is already populated.
pub fn batch_scan_typst_metadata(
    files: &[&PathBuf],
    root: &Path,
    label: &str,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<Vec<Option<JsonValue>>> {
    if files.is_empty() {
        return Ok(vec![]);
    }

    // Use visible-phase inputs for metadata scan so legacy templates relying on
    // `sys.inputs.format` keep working in query/bootstrap paths.
    let inputs = build_visible_inputs(config, store)?;
    let scanner = Batcher::for_scan(root)
        .with_inputs_obj(inputs)
        .with_snapshot_from(files)?;

    match scanner.batch_scan(files) {
        Ok(results) => {
            let mut metas = Vec::with_capacity(results.len());
            for (result, file) in results.into_iter().zip(files) {
                match result {
                    Ok(scan) => {
                        metas.push(scan.metadata(label));
                    }
                    Err(e) => {
                        // Retry with per-file @tola/current context so scans that
                        // evaluate current-dependent body expressions can still
                        // extract metadata.
                        match scan_single_with_current(root, file, config, store) {
                            Ok(scan) => metas.push(scan.metadata(label)),
                            Err(_) => {
                                let rel_path = file.strip_prefix(root).unwrap_or(file);
                                anyhow::bail!("failed to scan {}:\n{}", rel_path.display(), e);
                            }
                        }
                    }
                }
            }
            Ok(metas)
        }
        Err(e) => {
            anyhow::bail!("Batch scan failed: {}", e);
        }
    }
}

/// Batch scan Typst files for metadata with iterative support
///
/// Requires the page store to be pre-populated with all site pages.
/// Iteratively re-scans pages that use `@tola/pages` until convergence
pub fn batch_scan_typst_metadata_iterative(
    files: &[&PathBuf],
    root: &Path,
    label: &str,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<Vec<Option<JsonValue>>> {
    if files.is_empty() {
        return Ok(vec![]);
    }

    // Initial scan with visible-phase site/pages inputs.
    let inputs = build_visible_inputs(config, store)?;
    let scanner = Batcher::for_scan(root)
        .with_inputs_obj(inputs)
        .with_snapshot_from(files)?;
    let initial_results = scanner.batch_scan(files)?;

    let mut metas: Vec<Option<JsonValue>> = Vec::with_capacity(files.len());
    let mut iterative_indices: Vec<usize> = Vec::new();

    // Process initial results and identify iterative pages
    for (i, (result, file)) in initial_results.into_iter().zip(files.iter()).enumerate() {
        match result {
            Ok(scan) => {
                let meta = scan.metadata(label);
                let kind = PageKind::from_packages(scan.accessed_packages());

                // Update page store with new metadata.
                if let Some(ref meta_json) = meta {
                    update_stored_page_from_meta(file, meta_json, config, store);
                }

                if kind.is_iterative() {
                    iterative_indices.push(i);
                }
                metas.push(meta);
            }
            Err(e) => {
                // Retry with per-file current context to handle pages that
                // reference @tola/current in body while scanning metadata.
                match scan_single_with_current(root, file, config, store) {
                    Ok(scan) => {
                        let meta = scan.metadata(label);
                        let kind = PageKind::from_packages(scan.accessed_packages());

                        if let Some(ref meta_json) = meta {
                            update_stored_page_from_meta(file, meta_json, config, store);
                        }

                        if kind.is_iterative() {
                            iterative_indices.push(i);
                        }
                        metas.push(meta);
                    }
                    Err(_) => {
                        let rel_path = file.strip_prefix(root).unwrap_or(file);
                        anyhow::bail!("failed to scan {}:\n{}", rel_path.display(), e);
                    }
                }
            }
        }
    }

    // If no iterative pages, return early (zero overhead path)
    if iterative_indices.is_empty() {
        return Ok(metas);
    }

    // Iterative re-scan until convergence
    let mut stability = HashStabilityTracker::with_oscillation_detection(store.pages_hash());

    for iteration in 0..MAX_METADATA_SCAN_ITERATIONS {
        // Re-scan iterative files one-by-one so each file gets its own
        // @tola/current context (especially `path` and current permalink).
        for &idx in &iterative_indices {
            let file = files[idx];
            let scan = match scan_single_with_current(root, file, config, store) {
                Ok(scan) => scan,
                Err(e) => {
                    let rel_path = file.strip_prefix(root).unwrap_or(file);
                    anyhow::bail!("failed to scan {}:\n{}", rel_path.display(), e);
                }
            };
            let meta = scan.metadata(label);

            // Update page store with new metadata.
            if let Some(ref meta_json) = meta {
                update_stored_page_from_meta(file, meta_json, config, store);
            }

            metas[idx] = meta;
        }

        // Check convergence
        match stability.decide(store.pages_hash(), iteration, MAX_METADATA_SCAN_ITERATIONS) {
            StabilityDecision::Converged => {
                crate::debug!("scan"; "converged after {} iteration(s)", iteration + 1);
                break;
            }
            StabilityDecision::Oscillating => {
                crate::log!(
                    "warning";
                    "scan metadata oscillating (cycle detected), stopping after {} iterations",
                    iteration + 1
                );
                break;
            }
            StabilityDecision::MaxIterationsReached => {
                crate::log!(
                    "warning";
                    "metadata did not converge after {} iterations",
                    MAX_METADATA_SCAN_ITERATIONS
                );
            }
            StabilityDecision::Continue => {}
        }
    }

    Ok(metas)
}

/// Update the page-store entry for a source file using metadata-derived permalink.
fn update_stored_page_from_meta(
    file: &Path,
    meta_json: &JsonValue,
    config: &SiteConfig,
    store: &StoredPageMap,
) {
    let Ok(page_meta) = serde_json::from_value::<PageMeta>(meta_json.clone()) else {
        return;
    };
    let _ = store.apply_meta_for_source(file, page_meta, config);
}

/// Parse metadata JSON to PageMeta, logging warning on failure.
fn parse_page_meta(meta_json: JsonValue, file: &Path) -> Option<PageMeta> {
    match serde_json::from_value::<PageMeta>(meta_json) {
        Ok(meta) => Some(meta),
        Err(e) => {
            crate::log!("warning"; "failed to parse metadata for {}: {}", file.display(), e);
            None
        }
    }
}

/// Populate page store from all content files.
///
/// Scans all Typst and Markdown files to build the global page store
/// Must be called before `batch_scan_typst_metadata_iterative`
pub fn populate_stored_pages(config: &SiteConfig, store: &StoredPageMap) -> Result<()> {
    use crate::compiler::collect_all_files;

    let root = crate::utils::path::normalize_path(config.get_root());
    let content_dir = root.join(&config.build.content);
    let label = &config.build.meta.label;

    // Collect all content files
    let all_files = collect_all_files(&content_dir);
    let content_files = filter_content_files(all_files);
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(&content_files);

    // Scan Typst files
    let typst_metas = batch_scan_typst_metadata(&typst_files, &root, label, config, store)?;

    for (file, meta_json) in typst_files.iter().zip(typst_metas) {
        if let Some(meta_json) = meta_json
            && let Some(page_meta) = parse_page_meta(meta_json, file)
        {
            let _ = store.apply_meta_for_source(file, page_meta, config);
        }
    }

    // Scan Markdown files
    for file in &markdown_files {
        if let Ok(result) = scan_markdown_file(file, config, store)
            && let Some(meta_json) = result.raw_meta
            && let Some(page_meta) = parse_page_meta(meta_json, file)
        {
            let _ = store.apply_meta_for_source(file, page_meta, config);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;

    #[test]
    fn test_parallel_collection() {
        let collector = ParallelCollector::new();
        let items: Vec<i32> = (0..100).collect();

        items.par_iter().for_each(|&i| {
            collector.push(i * 2);
        });

        let mut results = collector.drain();
        results.sort();

        let expected: Vec<i32> = (0..100).map(|i| i * 2).collect();
        assert_eq!(results, expected);
    }

    #[test]
    fn test_empty_collector() {
        let collector: ParallelCollector<i32> = ParallelCollector::new();
        let results = collector.drain();
        assert!(results.is_empty());
    }

    #[test]
    fn test_drain_with_capacity() {
        let collector = ParallelCollector::new();
        collector.push(1);
        collector.push(2);
        collector.push(3);

        let results = collector.drain_with_capacity(10);
        assert_eq!(results.len(), 3);
    }
}
