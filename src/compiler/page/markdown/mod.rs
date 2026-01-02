//! Markdown format support for tola-ssg.
//!
//! This module contains all Markdown-specific functionality:
//!
//! - [`Markdown`] - PageFormat implementation
//! - [`convert`] - Markdown → tola-vdom VDOM conversion via `pulldown-cmark`
//! - [`filter`] - Draft filtering

mod compile;
pub mod convert;
mod filter;
mod scan;

use std::path::Path;

use anyhow::Result;

use crate::compiler::CompileContext;
use super::{PageCompileOutput, PageFormat, PageScanOutput};

// Re-export utilities
pub use convert::{MarkdownMetaExtractor, MarkdownOptions, from_markdown};
pub use filter::filter_drafts as filter_markdown_drafts;

// =============================================================================
// Markdown Format
// =============================================================================

/// Markdown format adapter.
pub struct Markdown;

impl PageFormat for Markdown {
    fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput> {
        compile::compile(path, ctx)
    }

    fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput> {
        scan::scan(path, ctx)
    }
}
