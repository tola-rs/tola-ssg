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

/// Trait for content format adapters
///
/// Each format (Typst, Markdown) implements this trait to provide
/// unified compilation and scanning capabilities
pub trait PageFormat {
    /// Compile content to HTML via VDOM pipeline.
    fn compile(path: &Path, ctx: &CompileContext<'_>) -> Result<PageCompileOutput>;

    /// Scan content for metadata and links (lightweight, no HTML rendering).
    fn scan(path: &Path, ctx: &CompileContext<'_>) -> Result<PageScanOutput>;
}

// =============================================================================
// ScannedPage (Unified for all formats)
// =============================================================================

/// Pre-scanned page data from draft filtering phase
///
/// Used by both Typst and Markdown to store metadata collected during
/// the initial scan, avoiding redundant parsing during compilation
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

/// A heading extracted from the document during scan
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

/// Result of draft filtering operation
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

/// Trait for filtering draft content files
///
/// Each format implements this to filter out draft files before compilation
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

/// Result of filtering drafts from both Typst and Markdown files
///
/// Contains pre-scanned page data (metadata + kind) for all non-draft files
pub struct DraftFilterResult<'a> {
    /// Typst batcher for reuse in compilation (internal).
    pub(super) batcher: Option<super::TypstBatcher<'a>>,
    /// Pre-scanned page data for all non-draft files (Typst + Markdown).
    pub scanned: Vec<ScannedPage>,
    /// Total number of draft files filtered out.
    pub drafts_skipped: usize,
    /// Errors encountered during scan phase.
    pub errors: Vec<(std::path::PathBuf, typst_batch::CompileError)>,
}

impl<'a> DraftFilterResult<'a> {
    /// Create an empty result (no drafts filtered, no pre-scan).
    pub fn empty() -> Self {
        Self {
            batcher: None,
            scanned: vec![],
            drafts_skipped: 0,
            errors: vec![],
        }
    }

    /// Take the batcher for Typst compilation.
    pub(super) fn take_batcher(&mut self) -> Option<super::TypstBatcher<'a>> {
        self.batcher.take()
    }

    /// Check if there are any errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Report errors and return an error if any exist.
    ///
    /// This centralizes error reporting logic for scan phase errors.
    pub fn report_errors(&self, max_errors: usize) -> anyhow::Result<()> {
        if self.errors.is_empty() {
            return Ok(());
        }

        for (_path, error) in self.errors.iter().take(max_errors) {
            let err = super::format_compile_error(error, max_errors);
            eprintln!("{}", err);
        }

        let total_errors = self.errors.len();
        if total_errors > max_errors {
            eprintln!("... and {} more errors", total_errors - max_errors);
        }

        Err(anyhow::anyhow!(
            "Scan failed with {} error(s)",
            total_errors
        ))
    }
}

// =============================================================================
// Unified Draft Filtering
// =============================================================================

use crate::config::SiteConfig;
use crate::core::ContentKind;

/// Filter draft files from both Typst and Markdown content
///
/// This is the unified entry point for draft filtering, used by both
/// `build` and `serve` modes. Returns a `DraftFilterResult` which can
/// report errors via `report_errors()`
pub fn filter_drafts<'a>(
    config: &'a SiteConfig,
    typst_files: &[&PathBuf],
    markdown_files: &[&PathBuf],
) -> DraftFilterResult<'a> {
    let root = config.get_root();
    let label = &config.build.meta.label;

    let typst_result = super::filter_typst_drafts(typst_files, root, label);
    let md_result = super::filter_markdown_drafts(markdown_files, root, label);
    let drafts_skipped = typst_result.draft_count + md_result.draft_count;

    DraftFilterResult {
        batcher: typst_result.batcher,
        scanned: [typst_result.scanned, md_result.scanned].concat(),
        drafts_skipped,
        errors: typst_result.errors,
    }
}

// =============================================================================
// Single Page Scan (for hot reload)
// =============================================================================

/// Scanned data from a single page (headings + links)
#[derive(Debug, Clone, Default)]
pub struct SinglePageScanData {
    /// Document headings.
    pub headings: Vec<ScannedHeading>,
    /// Internal page links (site-root only).
    pub links: Vec<String>,
}

/// Scan a single page to extract headings and links.
///
/// This is used by hot reload mode to update `@tola/current` data
/// before compilation. Dispatches to format-specific scan logic.
pub fn scan_single_page(path: &Path, config: &SiteConfig) -> SinglePageScanData {
    let kind = match ContentKind::from_path(path) {
        Some(k) => k,
        None => return SinglePageScanData::default(),
    };

    match kind {
        ContentKind::Typst => scan_typst_page(path, config),
        ContentKind::Markdown => scan_markdown_page(path),
    }
}

/// Scan a Typst page using typst_batch::Scanner (Eval-only, fast)
fn scan_typst_page(path: &Path, config: &SiteConfig) -> SinglePageScanData {
    use typst_batch::prelude::*;

    let root = config.get_root();
    let scan = match Scanner::new(root).scan(path) {
        Ok(s) => s,
        Err(_) => return SinglePageScanData::default(),
    };

    let headings = scan
        .headings()
        .into_iter()
        .map(|h| ScannedHeading {
            level: h.level,
            text: h.text,
            supplement: h.supplement,
        })
        .collect();

    let links = scan
        .links()
        .into_iter()
        .filter(|link| link.is_site_root())
        .map(|link| link.dest)
        .collect();

    SinglePageScanData { headings, links }
}

/// Scan a Markdown page (reuses filter.rs logic)
fn scan_markdown_page(path: &Path) -> SinglePageScanData {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return SinglePageScanData::default(),
    };

    SinglePageScanData {
        headings: super::markdown::extract_headings(&content),
        links: super::markdown::extract_links(&content),
    }
}
