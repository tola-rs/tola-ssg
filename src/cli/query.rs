//! Query command implementation.
//!
//! Extracts metadata from content files in batch using parallel processing.
//! Uses fast scanning for Typst files (5-20x faster) and shared VDOM pipeline for Markdown.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;
use serde::Serialize;
use serde_json::{Map, Value as JsonValue};

use super::common::{
    ParallelCollector, batch_scan_typst_metadata_iterative, calculate_url_path,
    collect_content_files, populate_stored_pages, scan_markdown_file,
};
use crate::cli::args::QueryArgs;
use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::log;
use crate::page::PageMeta;
use crate::utils::plural_count;

/// Metadata that can be either normalized or raw
#[derive(Debug)]
pub enum QueryMeta {
    Normalized(Box<PageMeta>),
    Raw(JsonValue),
}

impl QueryMeta {
    /// Check if this is a draft.
    fn is_draft(&self) -> bool {
        match self {
            QueryMeta::Normalized(meta) => meta.draft,
            QueryMeta::Raw(json) => json.get("draft").and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
}

impl Serialize for QueryMeta {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            QueryMeta::Normalized(meta) => meta.serialize(serializer),
            QueryMeta::Raw(json) => json.serialize(serializer),
        }
    }
}

/// Result for a single queried page
#[derive(Debug, Serialize)]
pub struct PageQueryResult {
    pub path: String,
    pub url: String,
    #[serde(flatten)]
    pub meta: QueryMeta,
}

/// Result for batch query
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct QueryResult {
    pub pages: Vec<PageQueryResult>,
}

/// Execute query command
pub fn run_query(args: &QueryArgs, config: &SiteConfig) -> Result<()> {
    // Register VFS for @tola/* virtual packages (no font warmup needed)
    crate::compiler::page::typst::init::init_vfs();

    // Populate STORED_PAGES with all site pages first
    // This ensures pages() returns correct data for all pages
    populate_stored_pages(config)?;

    let files = collect_content_files(&args.paths, &config.build.content)?;

    let file_count = files.len();
    log!("query"; "querying {}", plural_count(file_count, "file"));

    let results = query_files(&files, args, config)?;

    log!("query"; "found {}", plural_count(results.pages.len(), "page with metadata"));

    output_results(&results, args)?;
    Ok(())
}

fn query_files(files: &[PathBuf], args: &QueryArgs, config: &SiteConfig) -> Result<QueryResult> {
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

// ============================================================================
// Markdown Query (using shared VDOM pipeline)
// ============================================================================

/// Query Markdown file metadata using shared VDOM pipeline
fn query_markdown_vdom(file: &Path, config: &SiteConfig) -> Result<Option<JsonValue>> {
    let result = scan_markdown_file(file, config)?;
    Ok(result.raw_meta)
}

// ============================================================================
// Output Formatting
// ============================================================================

fn output_results(results: &QueryResult, args: &QueryArgs) -> Result<()> {
    // Skip output if no results
    if results.pages.is_empty() {
        return Ok(());
    }

    let output = if let Some(ref fields) = args.fields {
        filter_fields(results, fields, args.filter_empty)
    } else {
        format_results(results, args.filter_empty)
    };

    // Format output: JSON for --raw, simplified JSON for default
    let output_json = if args.raw {
        output
    } else {
        use typst_batch::codegen::json_to_simple_text;
        json_to_simple_text(&output)
    };

    let formatted = if args.pretty {
        serde_json::to_string_pretty(&output_json)?
    } else {
        serde_json::to_string(&output_json)?
    };

    // Output to file or stdout
    if let Some(ref output_path) = args.output {
        let mut file = fs::File::create(output_path)?;
        writeln!(file, "{}", formatted)?;
        log!("query"; "wrote output to {}", output_path.display());
    } else {
        println!("{}", formatted);
    }

    Ok(())
}

/// Format all results, optionally filtering empty fields
fn format_results(results: &QueryResult, filter_empty: bool) -> JsonValue {
    let pages: Vec<JsonValue> = results
        .pages
        .iter()
        .map(|page| format_page(page, filter_empty))
        .collect();

    JsonValue::Array(pages)
}

/// Format a single page result with path/url first
fn format_page(page: &PageQueryResult, filter_empty: bool) -> JsonValue {
    let mut obj = Map::new();

    // path and url always first
    obj.insert("path".to_string(), JsonValue::String(page.path.clone()));
    obj.insert("url".to_string(), JsonValue::String(page.url.clone()));

    // Add meta fields
    let meta_value = serde_json::to_value(&page.meta).unwrap_or_default();
    if let JsonValue::Object(meta_obj) = meta_value {
        for (key, value) in meta_obj {
            if !filter_empty || !is_empty_value(&value) {
                obj.insert(key, value);
            }
        }
    }

    JsonValue::Object(obj)
}

/// Check if a JSON value is considered "empty" (null, "", or [])
fn is_empty_value(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => true,
        JsonValue::String(s) => s.is_empty(),
        JsonValue::Array(arr) => arr.is_empty(),
        _ => false,
    }
}

/// Filter to specific fields, with path/url always included first
fn filter_fields(results: &QueryResult, fields: &[String], filter_empty: bool) -> JsonValue {
    let pages: Vec<JsonValue> = results
        .pages
        .iter()
        .map(|page| {
            let mut obj = Map::new();

            // path and url always first
            obj.insert("path".to_string(), JsonValue::String(page.path.clone()));
            obj.insert("url".to_string(), JsonValue::String(page.url.clone()));

            let meta_value = serde_json::to_value(&page.meta).unwrap_or_default();
            if let JsonValue::Object(meta_obj) = meta_value {
                for field in fields {
                    if let Some(value) = meta_obj.get(field) {
                        if !filter_empty || !is_empty_value(value) {
                            obj.insert(field.clone(), value.clone());
                        }
                    } else if !filter_empty {
                        // Field explicitly requested but doesn't exist - show null when not filtering
                        obj.insert(field.clone(), JsonValue::Null);
                    }
                }
            }

            JsonValue::Object(obj)
        })
        .collect();

    JsonValue::Array(pages)
}
