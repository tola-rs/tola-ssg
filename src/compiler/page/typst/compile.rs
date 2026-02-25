//! Typst content compilation to HTML.

use std::path::Path;

use anyhow::Result;
use typst_batch::prelude::*;

use crate::compiler::CompileContext;
use crate::compiler::page::{PageCompileOutput, format_compile_error};
use crate::page::{PageMeta, STORED_PAGES};
use crate::pipeline::compile as pipeline_compile;

use super::from_typst_html;

/// Parse metadata JSON to PageMeta, logging warning on failure.
fn parse_page_meta(json: serde_json::Value) -> Option<PageMeta> {
    match serde_json::from_value::<PageMeta>(json) {
        Ok(meta) => Some(meta),
        Err(e) => {
            crate::log!("warning"; "failed to parse metadata: {}", e);
            None
        }
    }
}

/// Compile a Typst file to HTML
///
/// For single-page compilation (watch mode), this injects both:
/// - `@tola/site` and `@tola/pages` via `build_inputs()`
/// - `@tola/current` via `build_current_context()` (if route is available)
pub fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput> {
    let root = ctx.config.get_root();
    let label = &ctx.config.build.meta.label;
    let max_errors = ctx
        .config
        .build
        .diagnostics
        .max_errors
        .unwrap_or(usize::MAX);

    // Build inputs with site config and pages data for @tola/* virtual packages
    let mut inputs = STORED_PAGES.build_inputs(ctx.config)?;

    // Inject @tola/current context if route is available
    if let Some(route) = ctx.route {
        // Get source path relative to content directory
        let source = route
            .source
            .strip_prefix(&ctx.config.build.content)
            .ok()
            .map(|p| p.to_string_lossy().to_string());

        let current_context =
            STORED_PAGES.build_current_context(&route.permalink, source.as_deref());
        inputs.merge_json(&current_context)?;
    }

    // Compile Typst to HtmlDocument using Builder API with inputs
    let result = Compiler::new(root)
        .with_inputs_obj(inputs)
        .with_path(path)
        .compile()
        .map_err(|e| format_compile_error(&e, max_errors))?;

    process_result(result, label, ctx)
}

/// Process a pre-compiled Typst result through the VDOM pipeline
///
/// This is used by batch compilation to process `Batcher` results
/// The Typst compilation has already been done; this handles:
/// - Warning filtering
/// - Metadata extraction
/// - VDOM conversion and processing
pub fn process_result(
    result: CompileResult,
    label: &str,
    ctx: &CompileContext<'_>,
) -> Result<PageCompileOutput> {
    // Filter warnings
    let warnings = result.diagnostics().filter_out(&[
        DiagnosticFilter::new(DiagnosticSeverity::Warning, FilterType::HtmlExport),
        DiagnosticFilter::new(
            DiagnosticSeverity::Warning,
            FilterType::Package(PackageKind::AllPreview),
        ),
    ]);

    // Extract parts
    let (document, accessed, _) = result.into_parts();

    // Extract and convert metadata (JsonValue → PageMeta)
    let meta: Option<PageMeta> = document.query_metadata(label).and_then(parse_page_meta);

    // Get global_header from metadata (default: true)
    let global_header = meta.as_ref().is_none_or(|m| m.global_header);

    // Convert to Raw VDOM
    let raw_doc = from_typst_html(&document, ctx.config.build.svg.baseline_align);

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
        accessed_files: accessed.files,
        accessed_packages: accessed.packages,
        warnings,
    })
}
