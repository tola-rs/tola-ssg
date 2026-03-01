use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;
use serde_json::Value as JsonValue;

use crate::cli::args::QueryArgs;
use crate::cli::common::{
    ParallelCollector, batch_scan_typst_metadata_iterative, calculate_url_path, scan_markdown_file,
};
use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::core::UrlPath;
use crate::log;
use crate::page::STORED_PAGES;
use crate::utils::path::normalize_path;

use super::types::{PageQueryResult, QueryMeta, QueryResult};

pub(super) fn query_files(
    files: &[PathBuf],
    args: &QueryArgs,
    config: &SiteConfig,
) -> Result<QueryResult> {
    // Normalize root to absolute
    let root = normalize_path(config.get_root());
    let include_drafts = args.drafts;
    let raw_mode = args.raw;
    let label = &config.build.meta.label;

    // Separate Typst and Markdown files
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(files);

    // Batch scan Typst files (with iterative support for pages() in metadata)
    let typst_results = batch_scan_typst_metadata_iterative(&typst_files, &root, label, config)?;

    // Lock-free parallel collection using SegQueue
    let collector = ParallelCollector::new();
    let file_count = files.len();

    // Process Typst results
    for (file, raw_meta) in typst_files.iter().zip(typst_results) {
        if let Some(result) = process_query_result(file, raw_meta, raw_mode, config) {
            if result.meta.is_draft() && !include_drafts {
                continue;
            }
            collector.push(result);
        }
    }

    // Process Markdown files in parallel
    markdown_files
        .par_iter()
        .for_each(|file| match query_markdown_vdom(file, config) {
            Ok(raw_meta) => {
                if let Some(result) = process_query_result(file, raw_meta, raw_mode, config) {
                    if result.meta.is_draft() && !include_drafts {
                        return;
                    }
                    collector.push(result);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to query {}: {}", file.display(), e);
            }
        });

    let pages = collector.drain_with_capacity(file_count);
    Ok(QueryResult { pages })
}

fn process_query_result(
    file: &Path,
    raw_meta: Option<JsonValue>,
    raw_mode: bool,
    config: &SiteConfig,
) -> Option<PageQueryResult> {
    let raw_meta = raw_meta?;
    let root = normalize_path(config.get_root());

    let rel_path = file
        .strip_prefix(&root)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();
    let permalink = resolve_permalink(file, &raw_meta, config);

    let meta = if raw_mode {
        QueryMeta::Raw(raw_meta)
    } else {
        match serde_json::from_value(raw_meta) {
            Ok(content_meta) => QueryMeta::Normalized(Box::new(content_meta)),
            Err(e) => {
                log!("warning"; "failed to normalize metadata for {}: {}", file.display(), e);
                return None;
            }
        }
    };

    Some(PageQueryResult {
        path: rel_path,
        permalink,
        meta,
    })
}

/// Resolve output permalink for query result.
///
/// Priority:
/// 1. Custom permalink from metadata (`permalink` field)
/// 2. Source -> permalink mapping from STORED_PAGES
/// 3. Computed default route from source path
/// 4. Legacy fallback path calculation
fn resolve_permalink(file: &Path, raw_meta: &JsonValue, config: &SiteConfig) -> String {
    // Respect explicit custom permalink from metadata.
    if let Some(custom) = raw_meta.get("permalink").and_then(|v| v.as_str()) {
        return UrlPath::from_page(custom).to_string();
    }

    // Prefer source mapping populated by `populate_stored_pages` (includes derived permalinks).
    if let Some(mapped) = STORED_PAGES.get_permalink_by_source(file) {
        return mapped.to_string();
    }

    // Keep behavior aligned with build routing (slug/path_prefix aware).
    if let Ok(compiled) = crate::compiler::page::CompiledPage::from_paths(file, config) {
        return compiled.route.permalink.to_string();
    }

    // Last-resort fallback for unexpected path/config errors.
    let content_dir = normalize_path(&config.build.content);
    calculate_url_path(file, &content_dir)
}

/// Query Markdown file metadata using shared VDOM pipeline
fn query_markdown_vdom(file: &Path, config: &SiteConfig) -> Result<Option<JsonValue>> {
    let result = scan_markdown_file(file, config)?;
    Ok(result.raw_meta)
}
