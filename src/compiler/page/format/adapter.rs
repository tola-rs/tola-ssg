//! Format adapter trait.

use std::path::Path;

use anyhow::Result;

use crate::compiler::CompileContext;

use super::super::output::{PageCompileOutput, PageScanOutput};

/// Trait for content format adapters.
///
/// Each format implements this trait to provide unified compilation and
/// lightweight scanning capabilities.
pub trait PageFormat {
    /// Compile content to HTML via VDOM pipeline.
    fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput>;

    /// Scan content for metadata and links without full HTML rendering.
    fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput>;
}
