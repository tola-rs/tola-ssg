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
use crate::log;

use super::types::{PageQueryResult, QueryMeta, QueryResult};

pub(super) fn query_files(
    files: &[PathBuf],
    args: &QueryArgs,
    config: &SiteConfig,
) -> Result<QueryResult> {
    // Normalize root to absolute, then derive content_dir from it
    let root = crate::utils::path::normalize_path(config.get_root());
    let content_dir = root.join(&config.build.content);
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
        if let Some(result) = process_query_result(file, raw_meta, &root, &content_dir, raw_mode) {
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
                if let Some(result) =
                    process_query_result(file, raw_meta, &root, &content_dir, raw_mode)
                {
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
    root: &Path,
    content_dir: &Path,
    raw_mode: bool,
) -> Option<PageQueryResult> {
    let raw_meta = raw_meta?;

    let rel_path = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();
    let url = calculate_url_path(file, content_dir);

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
        url,
        meta,
    })
}

/// Query Markdown file metadata using shared VDOM pipeline
fn query_markdown_vdom(file: &Path, config: &SiteConfig) -> Result<Option<JsonValue>> {
    let result = scan_markdown_file(file, config)?;
    Ok(result.raw_meta)
}
