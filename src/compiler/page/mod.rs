//! Page compilation and processing.
//!
//! - [`compile`] / [`scan`] - Unified entry points for any content format
//! - [`process_page`] - Single page compilation for watch mode
//! - [`build_static_pages`] - Compile all pages, write static after conflict check
//! - [`rebuild_iterative_pages`] - Recompile iterative pages with complete data
//!
//! # Module Structure
//!
//! - [`format`] - PageFormat trait for format adapters
//! - [`output`] - Compilation output types (PageCompileOutput, PageScanOutput)
//! - [`cache`] - Build VDOM cache
//! - [`warning`] - Compilation warnings collection
//! - [`process`] - Build and compile processes (batch, single, conflict)
//! - [`markdown`] - Markdown format implementation
//! - [`typst`] - Typst format implementation

mod cache;
mod format;
pub mod markdown;
mod output;
mod process;
pub mod typst;
mod warning;
mod write;

use std::path::Path;

use anyhow::Result;

use crate::compiler::CompileContext;
use crate::core::ContentKind;

// Re-export types
pub use cache::{BUILD_CACHE, IndexedDocument, cache_vdom};
pub use format::{DraftFilterResult, PageFormat, ScannedHeading, ScannedPage, filter_drafts};
pub use markdown::{Markdown, filter_markdown_drafts};
pub use output::{PageCompileOutput, PageScanOutput};
pub use process::collect_content_files;
pub use process::process_page;
pub use process::{
    build_address_space, build_static_pages, populate_pages, rebuild_iterative_pages,
};
pub use typst::process_result as process_typst_result;
pub use typst::{Typst, filter_drafts as filter_typst_drafts};
pub use warning::{collect_warnings, drain_warnings};
pub use write::{write_page_html, write_redirects};

// Re-export page domain types
pub use crate::page::{CompiledPage, PageRoute, Pages};

// ============================================================================
// Unified Compilation API
// ============================================================================

/// Compile any content file to HTML using the VDOM pipeline
///
/// Dispatches to the appropriate format adapter based on file extension
pub fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput> {
    let kind = ContentKind::from_path(path)
        .ok_or_else(|| anyhow::anyhow!("unsupported content type: {:?}", path))?;

    match kind {
        ContentKind::Typst => typst::Typst::compile(path, ctx),
        ContentKind::Markdown => markdown::Markdown::compile(path, ctx),
    }
}

/// Scan any content file to Indexed VDOM (lightweight, no HTML rendering)
///
/// Faster than `compile()` for validation/query use cases
pub fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput> {
    let kind = ContentKind::from_path(path)
        .ok_or_else(|| anyhow::anyhow!("unsupported content type: {:?}", path))?;

    match kind {
        ContentKind::Typst => typst::Typst::scan(path, ctx),
        ContentKind::Markdown => markdown::Markdown::scan(path, ctx),
    }
}

// ============================================================================
// Type Aliases
// ============================================================================

/// Result of compile_meta: (html, metadata, indexed_vdom)
#[allow(dead_code)]
pub type CompileMetaResult = (
    Vec<u8>,
    Option<crate::page::PageMeta>,
    Option<IndexedDocument>,
);

/// Typst batch compiler (reusable for lock-free snapshot)
pub type TypstBatcher<'a> = typst_batch::Batcher<'a>;

/// Typst file snapshot for reuse across compilation phases
pub type FileSnapshot = std::sync::Arc<typst_batch::FileSnapshot>;

/// Result of batch compilation for a single file
pub type BatchCompileResult =
    std::result::Result<typst_batch::CompileResult, typst_batch::CompileError>;

/// Format a CompileError with max_errors limit from config
///
/// This limits the number of errors displayed to avoid cascading error spam
/// from a single syntax error
pub fn format_compile_error(error: &typst_batch::CompileError, max_errors: usize) -> anyhow::Error {
    match error.diagnostics() {
        Some(diags) => anyhow::anyhow!("{}", diags.with_max_errors(max_errors)),
        None => anyhow::anyhow!("{}", error),
    }
}

/// Compilation statistics: counts of direct, iterative, and skipped draft pages
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct CompileStats {
    /// Number of direct pages written.
    pub direct_pages: usize,
    /// Number of iterative pages (may need recompilation).
    pub iterative_pages: usize,
    /// Number of draft pages skipped.
    pub drafts_skipped: usize,
}

impl CompileStats {
    /// Create new stats with the given counts.
    #[inline]
    pub fn new(direct_pages: usize, iterative_pages: usize, drafts_skipped: usize) -> Self {
        Self {
            direct_pages,
            iterative_pages,
            drafts_skipped,
        }
    }

    /// Check if any drafts were skipped.
    #[inline]
    pub fn has_skipped_drafts(&self) -> bool {
        self.drafts_skipped > 0
    }

    /// Total number of pages (direct + iterative).
    #[inline]
    #[allow(dead_code)]
    pub fn total(&self) -> usize {
        self.direct_pages + self.iterative_pages
    }
}

/// Result of collect_metadata: paths of iterative pages, stats, and reusable snapshot
///
/// The snapshot can be passed to `rebuild_iterative_pages` for lock-free reuse
pub struct MetadataResult {
    /// Paths of pages requiring iterative compilation (use @tola/pages or @tola/current).
    pub iterative_paths: Vec<std::path::PathBuf>,
    /// Compilation statistics.
    pub stats: CompileStats,
    /// Reusable Typst file snapshot.
    pub snapshot: Option<FileSnapshot>,
}

impl MetadataResult {
    /// Check if there are iterative pages requiring recompilation.
    #[inline]
    pub fn has_iterative_pages(&self) -> bool {
        !self.iterative_paths.is_empty()
    }
}

/// Result of page compilation (watch mode)
pub struct PageResult {
    pub page: CompiledPage,
    pub indexed_vdom: Option<IndexedDocument>,
    pub permalink: crate::core::UrlPath,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use crate::compiler::page::CompiledPage;
    use crate::config::SiteConfig;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Create a test config with the given content directory.
    fn make_test_config(content_dir: PathBuf, output_dir: PathBuf) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir;
        config
    }

    #[test]
    fn test_page_meta_from_paths() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        // Create a dummy file
        let file_path = content_dir.join("test.typ");
        fs::write(&file_path, "= Test").unwrap();

        let config = make_test_config(content_dir.clone(), output_dir);
        let page = CompiledPage::from_paths(file_path, &config);

        assert!(page.is_ok());
        let page = page.unwrap();
        assert_eq!(page.route.relative, "test");
        assert!(page.route.output_file.ends_with("test/index.html"));
    }

    #[test]
    fn test_page_meta_nested_path() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(content_dir.join("posts")).unwrap();

        let file_path = content_dir.join("posts/hello.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let config = make_test_config(content_dir.clone(), output_dir);
        let page = CompiledPage::from_paths(file_path, &config);

        assert!(page.is_ok());
        let page = page.unwrap();
        assert_eq!(page.route.relative, "posts/hello");
    }
}
