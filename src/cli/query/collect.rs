use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;
use serde_json::Value as JsonValue;

use crate::cli::args::QueryArgs;
use crate::cli::common::{
    ParallelCollector, batch_scan_typst_metadata_iterative, scan_markdown_file,
};
use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::core::UrlPath;
use crate::log;
use crate::page::StoredPageMap;
use crate::utils::path::normalize_path;
use crate::utils::path::route::strip_path_prefix_from_page_url;

use super::types::{PageQueryResult, QueryMeta, QueryResult};

pub(super) fn query_files(
    files: &[PathBuf],
    args: &QueryArgs,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<QueryResult> {
    // Normalize root to absolute
    let root = normalize_path(config.get_root());
    let include_drafts = args.drafts;
    let raw_mode = args.raw;
    let label = &config.build.meta.label;

    // Separate Typst and Markdown files
    let (typst_files, markdown_files) = ContentKind::partition_by_kind(files);

    // Batch scan Typst files (with iterative support for pages() in metadata)
    let typst_results =
        batch_scan_typst_metadata_iterative(&typst_files, &root, label, config, store)?;

    // Lock-free parallel collection using SegQueue
    let collector = ParallelCollector::new();
    let file_count = files.len();

    // Process Typst results
    for (file, raw_meta) in typst_files.iter().zip(typst_results) {
        if let Some(result) = process_query_result(file, raw_meta, raw_mode, config, store) {
            if result.meta.is_draft() && !include_drafts {
                continue;
            }
            collector.push(result);
        }
    }

    // Process Markdown files in parallel
    markdown_files
        .par_iter()
        .for_each(|file| match query_markdown_vdom(file, config, store) {
            Ok(raw_meta) => {
                if let Some(result) = process_query_result(file, raw_meta, raw_mode, config, store)
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
    raw_mode: bool,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Option<PageQueryResult> {
    let raw_meta = raw_meta?;
    let root = normalize_path(config.get_root());

    let rel_path = file
        .strip_prefix(&root)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();
    let permalink = match resolve_permalink(file, &raw_meta, config, store) {
        Ok(permalink) => permalink,
        Err(e) => {
            log!("warning"; "failed to resolve permalink for {}: {}", file.display(), e);
            return None;
        }
    };

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
/// 2. Source -> permalink mapping from the page store
/// 3. Computed default route from source path
fn resolve_permalink(
    file: &Path,
    raw_meta: &JsonValue,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<String> {
    let prefix = config.paths().prefix().to_string_lossy().into_owned();

    // Respect explicit custom permalink from metadata.
    if let Some(custom) = raw_meta.get("permalink").and_then(|v| v.as_str()) {
        return Ok(strip_path_prefix_from_page_url(
            UrlPath::from_page(custom).as_ref(),
            &prefix,
        ));
    }

    // Prefer source mapping populated by `populate_stored_pages` (includes derived permalinks).
    if let Some(mapped) = store.get_permalink_by_source(file) {
        return Ok(strip_path_prefix_from_page_url(mapped.as_str(), &prefix));
    }

    // Keep behavior aligned with build routing (slug/path_prefix aware).
    let compiled = crate::compiler::page::CompiledPage::from_paths(file, config)?;
    Ok(strip_path_prefix_from_page_url(
        compiled.route.permalink.as_str(),
        &prefix,
    ))
}

/// Query Markdown file metadata using shared VDOM pipeline
fn query_markdown_vdom(
    file: &Path,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<Option<JsonValue>> {
    let result = scan_markdown_file(file, config, store)?;
    Ok(result.raw_meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn query_result_rejects_unroutable_source_without_permalink() {
        let dir = TempDir::new().unwrap();
        let root = normalize_path(dir.path());
        let content_dir = root.join("content");
        let outside_dir = root.join("templates");
        std::fs::create_dir_all(&content_dir).unwrap();
        std::fs::create_dir_all(&outside_dir).unwrap();

        let source = outside_dir.join("post.typ");
        std::fs::write(&source, "= Post").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.build.content = content_dir;

        let store = StoredPageMap::new();
        let result = process_query_result(
            &source,
            Some(json!({ "title": "Post" })),
            true,
            &config,
            &store,
        );

        assert!(result.is_none());
    }
}
