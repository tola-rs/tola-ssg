//! Typst format support for tola-ssg.
//!
//! This module contains all Typst-specific functionality:
//!
//! - [`init`] - Tola-specific setup (VirtualFS, warmup)
//! - [`Typst`] - PageFormat implementation
//! - [`convert`] - `HtmlDocument` -> tola-vdom VDOM conversion
//! - [`filter`] - Draft filtering

mod compile;
pub mod convert;
mod filter;
pub mod init;
mod iterative;
mod scan;

use std::path::Path;

use anyhow::Result;

use super::{PageCompileOutput, PageFormat, PageScanOutput};
use crate::compiler::CompileContext;

// Re-export utilities
pub use compile::process_result;
pub use convert::from_typst_html;
pub use filter::filter_drafts;
pub use init::{build_nested_mappings, init_runtime, init_vfs};
pub use iterative::{MAX_METADATA_SCAN_ITERATIONS, scan_single_with_current};

// =============================================================================
// Typst Format
// =============================================================================

/// Typst format adapter
pub struct Typst;

impl PageFormat for Typst {
    fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput> {
        compile::compile(path, ctx)
    }

    fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput> {
        scan::scan(path, ctx)
    }
}
