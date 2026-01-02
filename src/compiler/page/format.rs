//! Page format trait definitions.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::compiler::CompileContext;
use crate::page::{PageKind, PageMeta};

use super::{PageCompileOutput, PageScanOutput};

// =============================================================================
// PageFormat Trait
// =============================================================================

/// Trait for content format adapters.
///
/// Each format (Typst, Markdown) implements this trait to provide
/// unified compilation and scanning capabilities.
pub trait PageFormat {
    /// Compile content to HTML via VDOM pipeline.
    fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput>;

    /// Scan content for metadata and links (lightweight, no HTML rendering).
    fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput>;
}

// =============================================================================
// ScannedPage (Unified for all formats)
// =============================================================================

/// Pre-scanned page data from draft filtering phase.
///
/// Used by both Typst and Markdown to store metadata collected during
/// the initial scan, avoiding redundant parsing during compilation.
#[derive(Debug, Clone)]
pub struct ScannedPage {
    /// Source file path.
    pub path: PathBuf,
    /// Page metadata from scan (may be None if no metadata found).
    pub meta: Option<PageMeta>,
    /// Page compilation kind (Direct or Iterative).
    pub kind: PageKind,
    /// Internal page links extracted during scan (site-root links only).
    /// Used to populate PAGE_LINKS before compilation.
    pub links: Vec<String>,
    /// Document headings extracted during scan.
    /// Used to populate @tola/current.headings.
    pub headings: Vec<ScannedHeading>,
}

/// A heading extracted from the document during scan.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScannedHeading {
    /// Heading level (1-6).
    pub level: u8,
    /// Heading text content.
    pub text: String,
    /// Heading supplement (e.g., custom section name).
    /// None if default "Section".
    pub supplement: Option<String>,
}

impl ScannedPage {
    /// Extract paths from scanned pages.
    pub fn paths(scanned: &[ScannedPage]) -> Vec<&PathBuf> {
        scanned.iter().map(|s| &s.path).collect()
    }

    /// Partition scanned pages by content kind.
    pub fn partition_by_kind(scanned: &[ScannedPage]) -> (Vec<&ScannedPage>, Vec<&ScannedPage>) {
        use crate::core::ContentKind;
        scanned
            .iter()
            .partition(|s| ContentKind::from_path(&s.path) == Some(ContentKind::Typst))
    }
}

// =============================================================================
// DraftFilter Trait
// =============================================================================

/// Result of draft filtering operation.
pub struct FilterResult<'a, T = ()> {
    /// Files that are not drafts.
    pub files: Vec<&'a PathBuf>,
    /// Number of draft files filtered out.
    pub draft_count: usize,
    /// Optional extra data (e.g., Typst batcher).
    pub extra: T,
}

impl<'a> FilterResult<'a, ()> {
    /// Create a filter result without extra data.
    pub fn new(files: Vec<&'a PathBuf>, draft_count: usize) -> Self {
        Self {
            files,
            draft_count,
            extra: (),
        }
    }
}

impl<'a, T> FilterResult<'a, T> {
    /// Create a filter result with extra data.
    pub fn with_extra(files: Vec<&'a PathBuf>, draft_count: usize, extra: T) -> Self {
        Self {
            files,
            draft_count,
            extra,
        }
    }
}

/// Trait for filtering draft content files.
///
/// Each format implements this to filter out draft files before compilation.
pub trait DraftFilter {
    /// Extra data returned from filtering (e.g., Typst batcher for reuse).
    type Extra;

    /// Filter out draft files from the given list.
    ///
    /// Returns non-draft files and the count of filtered drafts.
    fn filter_drafts<'a>(
        files: Vec<&'a PathBuf>,
        root: &'a Path,
        label: &str,
    ) -> FilterResult<'a, Self::Extra>;
}

// =============================================================================
// Combined Draft Filter Result
// =============================================================================

/// Result of filtering drafts from both Typst and Markdown files.
///
/// Contains pre-scanned page data (metadata + kind) for all non-draft files.
pub struct DraftFilterResult<'a> {
    /// Typst batcher for reuse in compilation (internal).
    pub(super) batcher: Option<super::TypstBatcher<'a>>,
    /// Pre-scanned page data for all non-draft files (Typst + Markdown).
    pub scanned: Vec<ScannedPage>,
    /// Total number of draft files filtered out.
    pub drafts_skipped: usize,
}

impl<'a> DraftFilterResult<'a> {
    /// Create an empty result (no drafts filtered, no pre-scan).
    pub fn empty() -> Self {
        Self {
            batcher: None,
            scanned: vec![],
            drafts_skipped: 0,
        }
    }

    /// Take the batcher for Typst compilation.
    pub(super) fn take_batcher(&mut self) -> Option<super::TypstBatcher<'a>> {
        self.batcher.take()
    }
}
