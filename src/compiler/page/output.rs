//! Output types for page compilation.
//!
//! These are the unified result types produced by all content formats.

use std::path::PathBuf;

use crate::compiler::family::Indexed;
use crate::page::{PageKind, PageMeta};
use typst_batch::Diagnostics;

// =============================================================================
// PageCompileOutput
// =============================================================================

/// Output from page compilation
///
/// All content types (Typst, Markdown) produce this same output structure,
/// enabling uniform handling regardless of source format
pub struct PageCompileOutput {
    /// Generated HTML bytes
    pub html: Vec<u8>,
    /// Indexed VDOM for diff comparison (only in development mode)
    pub indexed_vdom: Option<tola_vdom::Document<Indexed>>,
    /// Extracted metadata (unified across all formats)
    pub meta: Option<PageMeta>,
    /// Files accessed during compilation (for dependency tracking)
    pub accessed_files: Vec<PathBuf>,
    /// Packages accessed during compilation (for iterative page detection)
    pub accessed_packages: Vec<typst_batch::PackageId>,
    /// Compilation warnings (e.g., unknown font family)
    pub warnings: Diagnostics,
}

impl PageCompileOutput {
    /// Determine the page compilation kind based on accessed packages.
    ///
    /// Pages that import `@tola/pages` or `@tola/current` need iterative
    /// compilation because the data depends on all pages being scanned first.
    #[inline]
    pub fn page_kind(&self) -> PageKind {
        PageKind::from_packages(&self.accessed_packages)
    }
}

// =============================================================================
// PageScanOutput
// =============================================================================

/// Output from lightweight page scanning
///
/// Used for validation and query scenarios where full HTML rendering is not needed
/// This is a pure data carrier - business logic (like draft checking) belongs in CLI layer
pub struct PageScanOutput {
    /// Indexed VDOM for link/asset extraction
    pub indexed_vdom: tola_vdom::Document<Indexed>,
    /// Raw metadata as JSON (preserves original structure for --raw mode)
    pub raw_meta: Option<serde_json::Value>,
}
