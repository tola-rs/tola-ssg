//! VDOM processing pipeline.
//!
//! Transforms `Document<Raw>` into HTML through the VDOM phases.
//! This module is format-agnostic and knows nothing about Typst, Markdown, etc.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │         Compile Phase (rayon sync)       │
//! │  Raw -> Indexed -> Processed -> HTML        │
//! └──────────────────────────────────────────┘
//!
//! ┌──────────────────────────────────────────┐
//! │         Scan Phase (lightweight)         │
//! │  Raw -> Indexed (stop here)               │
//! └──────────────────────────────────────────┘
//! ```
//!
//! - `compile()`: Full pipeline for production builds
//! - `compile_for_scan()`: Lightweight pipeline for validation/query (stops at Indexed)
//!
//! Validation is handled separately in `cli/validate.rs`.

pub mod transform;

use tola_vdom::prelude::*;

use crate::compiler::CompileContext;
use crate::compiler::family::{IndexedDocument, Processed, ProcessedDocument, Raw, TolaSite};
use crate::compiler::page::PageRoute;

pub use transform::{BodyInjector, HeaderInjector, LinkTransform, MediaTransform, SvgTransform};

// =============================================================================
// Types
// =============================================================================

/// Result of the compilation pipeline
#[derive(Debug)]
pub struct CompileOutput {
    /// Rendered HTML bytes.
    pub html: Vec<u8>,
    /// Rendered body fragment for feed output.
    pub feed_body: Option<String>,
    /// Indexed VDOM for validation and hot reload diffing.
    /// Present when `mode.cache_vdom` is true OR validation is enabled.
    pub indexed: Option<IndexedDocument>,
    /// Document statistics.
    #[allow(dead_code)]
    pub stats: TolaSite::ProcessedDocExt,
}

// =============================================================================
// Compilation (Sync)
// =============================================================================

/// Compile a Raw VDOM document to HTML
///
/// This is a **synchronous** function for optimal performance with rayon
/// Validation is handled separately by `cli/validate.rs`
///
/// # Returns
///
/// - `html`: Rendered HTML bytes
/// - `indexed`: VDOM for validation (when `cache_vdom` or validation enabled)
/// - `stats`: Document statistics
pub fn compile(doc: Document<Raw>, ctx: &CompileContext<'_>) -> CompileOutput {
    let indexer = match ctx.permalink() {
        Some(path) => TolaSite::indexer().with_page_seed(PageSeed::from_path(path)),
        None => TolaSite::indexer(),
    };

    let mut indexed_cache = None;

    let default_route = PageRoute::default();
    let route = ctx.route.unwrap_or(&default_route);

    // Build pipeline (sync transforms only, no validation)
    let indexed = Pipeline::new(doc)
        .pipe(HeaderInjector::new(ctx.config).with_global_header(ctx.global_header))
        .pipe(indexer)
        .pipe(LinkTransform::new(ctx.config, route))
        .pipe(MediaTransform::new(ctx.config, route))
        // Transforms that affect diff/hotreload must be placed BEFORE this line
        .inspect_if(ctx.mode.cache_vdom, |doc| {
            indexed_cache = Some(doc.clone());
        })
        .pipe(SvgTransform::new(ctx.config, route, ctx.mode))
        .into_inner();

    let feed_source = ctx.feed_body.should_render().then(|| indexed.clone());
    let indexed = Pipeline::new(indexed)
        .pipe(BodyInjector::new(ctx.config))
        .into_inner();

    // Process and render
    let processed = Pipeline::new(indexed)
        .pipe(TolaSite::processor())
        .into_inner();

    let render_config = RenderConfig::new(ctx.mode.emit_ids, ctx.config.build.minify);
    let feed_body = feed_source.map(|doc| {
        let processed = Pipeline::new(doc).pipe(TolaSite::processor()).into_inner();
        render_feed_body(&processed, &render_config)
    });

    CompileOutput {
        html: render_document_bytes(&processed, &render_config),
        feed_body,
        indexed: indexed_cache,
        stats: processed.meta,
    }
}

fn render_feed_body(doc: &ProcessedDocument, config: &RenderConfig) -> String {
    match doc.root.tag.as_str() {
        "html" => doc
            .find(|elem| elem.tag == "body")
            .map(|body| render_children(&body.children, config))
            .unwrap_or_else(|| render_document(doc, config)),
        "body" => render_children(&doc.root.children, config),
        _ => render_document(doc, config),
    }
}

fn render_children(children: &[Node<Processed>], config: &RenderConfig) -> String {
    let mut wrapper = Element::<Processed>::new("div");
    wrapper.children.extend(children.iter().cloned());

    let html = render_document(&Document::new(wrapper), config);
    html.strip_prefix("<div>")
        .and_then(|html| html.strip_suffix("</div>"))
        .unwrap_or(&html)
        .to_string()
}

// =============================================================================
// Scan Mode (Lightweight)
// =============================================================================

/// Compile a Raw VDOM document to Indexed phase only
///
/// This is a **lightweight** pipeline that stops at the Indexed phase,
/// skipping LinkTransform, MediaTransform, Processor, and HTML rendering
///
/// Use this for:
/// - **Validation**: Extract links/assets without full rendering
/// - **Query**: Extract metadata without full rendering
///
/// # Performance
///
/// ~3-5x faster than `compile()` for validation/query scenarios
#[inline]
pub fn compile_for_scan(doc: Document<Raw>, ctx: &CompileContext<'_>) -> IndexedDocument {
    let indexer = match ctx.permalink() {
        Some(path) => TolaSite::indexer().with_page_seed(PageSeed::from_path(path)),
        None => TolaSite::indexer(),
    };

    Pipeline::new(doc)
        .pipe(HeaderInjector::new(ctx.config).with_global_header(ctx.global_header))
        .pipe(indexer)
        .into_inner()
}
