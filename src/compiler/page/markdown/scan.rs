//! Markdown content scanning (lightweight, no HTML rendering).

use std::path::Path;

use anyhow::Result;

use crate::compiler::CompileContext;
use crate::compiler::page::PageScanOutput;
use crate::pipeline::compile_for_scan;

use super::{MarkdownMetaExtractor, MarkdownOptions, from_markdown};

/// Scan a Markdown file to Indexed VDOM (no HTML rendering).
pub fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput> {
    // Read source file
    let source = std::fs::read_to_string(path)?;

    // Extract metadata from frontmatter
    let extractor = MarkdownMetaExtractor;
    let (meta, body) = match extractor.extract_frontmatter(&source)? {
        Some((meta, body)) => (Some(meta), body.to_string()),
        None => (None, source),
    };

    // Convert PageMeta to JSON for raw_meta
    let raw_meta = meta.and_then(|m| serde_json::to_value(m).ok());

    // Convert markdown to Raw VDOM
    let options = MarkdownOptions::all();
    let raw_doc = from_markdown(&body, &options);

    // Process through lightweight pipeline (stops at Indexed)
    let indexed_vdom = compile_for_scan(raw_doc, ctx);

    Ok(PageScanOutput {
        indexed_vdom,
        raw_meta,
    })
}
