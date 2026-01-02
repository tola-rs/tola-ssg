//! Markdown content compilation to HTML.

use std::path::Path;

use anyhow::Result;
use typst_batch::Diagnostics;

use crate::compiler::CompileContext;
use crate::compiler::page::PageCompileOutput;
use crate::pipeline::compile as pipeline_compile;

use super::{MarkdownMetaExtractor, MarkdownOptions, from_markdown};

/// Compile a Markdown file to HTML.
pub fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput> {
    // Read source file
    let source = std::fs::read_to_string(path)?;

    // Extract metadata from frontmatter
    let extractor = MarkdownMetaExtractor;
    let (meta, body) = match extractor.extract_frontmatter(&source)? {
        Some((meta, body)) => (Some(meta), body.to_string()),
        None => (None, source),
    };

    // Get global_header from metadata (default: true)
    let global_header = meta.as_ref().is_none_or(|m| m.global_header);

    // Convert markdown to Raw VDOM
    let options = MarkdownOptions::all();
    let raw_doc = from_markdown(&body, &options);

    // Create compile context with global_header setting
    let compile_ctx = CompileContext {
        global_header,
        ..*ctx
    };

    // Process through VDOM pipeline (sync, no validation)
    let output = pipeline_compile(raw_doc, &compile_ctx);

    Ok(PageCompileOutput {
        html: output.html,
        indexed_vdom: output.indexed,
        meta,
        accessed_files: vec![path.to_path_buf()],
        accessed_packages: vec![], // Markdown doesn't access packages
        warnings: Diagnostics::new(),
    })
}
