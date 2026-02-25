//! Typst content scanning (lightweight, no HTML rendering).

use std::path::Path;

use anyhow::Result;
use typst_batch::prelude::*;

use crate::compiler::CompileContext;
use crate::compiler::page::{PageScanOutput, format_compile_error};
use crate::pipeline::compile_for_scan;

use super::from_typst_html;

/// Scan a Typst file to Indexed VDOM (no HTML rendering)
///
/// NOTE: For validate/query use cases, prefer `typst_batch::Scanner`
/// which is faster by skipping the Layout phase entirely
/// This function still performs full Typst compilation for VDOM conversion
pub fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput> {
    let root = ctx.config.get_root();
    let label = &ctx.config.build.meta.label;
    let max_errors = ctx
        .config
        .build
        .diagnostics
        .max_errors
        .unwrap_or(usize::MAX);

    // Compile Typst to HtmlDocument using Builder API
    let result = Compiler::new(root)
        .with_path(path)
        .compile()
        .map_err(|e| format_compile_error(&e, max_errors))?;

    let (document, _, _) = result.into_parts();

    // Extract raw metadata as JSON (preserves original Typst structure)
    let raw_meta = document.query_metadata(label);

    // Convert to Raw VDOM (scan doesn't render HTML, skip baseline calculation)
    let raw_doc = from_typst_html(&document, false);

    // Process through lightweight pipeline (stops at Indexed)
    let indexed_vdom = compile_for_scan(raw_doc, ctx);

    Ok(PageScanOutput {
        indexed_vdom,
        raw_meta,
    })
}
