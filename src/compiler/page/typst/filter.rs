//! Draft filtering for Typst files.

use std::path::{Path, PathBuf};

use typst_batch::prelude::*;

use crate::compiler::page::format::{DraftFilter, FilterResult, ScannedHeading, ScannedPage};
use crate::compiler::page::{Typst, TypstBatcher};
use crate::package::Phase;
use crate::page::{PageKind, PageMeta};

/// Result of Typst draft filtering, includes batcher for reuse
pub struct TypstFilterResult<'a> {
    /// Number of draft files filtered out.
    pub draft_count: usize,
    /// Batcher for reuse in subsequent compilation.
    pub batcher: Option<TypstBatcher<'a>>,
    /// Pre-scanned page data (metadata + kind) for non-draft files.
    pub scanned: Vec<ScannedPage>,
    /// Errors encountered during scan phase.
    pub errors: Vec<(PathBuf, typst_batch::CompileError)>,
}

impl<'a> TypstFilterResult<'a> {
    fn new(
        draft_count: usize,
        batcher: Option<TypstBatcher<'a>>,
        scanned: Vec<ScannedPage>,
        errors: Vec<(PathBuf, typst_batch::CompileError)>,
    ) -> Self {
        Self {
            draft_count,
            batcher,
            scanned,
            errors,
        }
    }

    fn empty(draft_count: usize, batcher: Option<TypstBatcher<'a>>) -> Self {
        Self::new(draft_count, batcher, vec![], vec![])
    }
}

/// Filter Typst files, removing drafts
///
/// Also collects metadata and detects iterative pages (importing @tola/pages or @tola/current)
/// for pre-scan optimization
///
/// Returns batcher for reuse in subsequent compilation phases
pub fn filter_drafts<'a>(files: &[&PathBuf], root: &'a Path, label: &str) -> TypstFilterResult<'a> {
    if files.is_empty() {
        return TypstFilterResult::empty(0, None);
    }

    let batcher = match Compiler::new(root)
        .into_batch()
        .with_inputs([(Phase::input_key(), Phase::Filter.as_str())])
        .with_snapshot_from(files)
    {
        Ok(b) => b,
        Err(_) => return TypstFilterResult::empty(0, None), // On prepare error, include all
    };

    let scan_results = match batcher.batch_scan(files) {
        Ok(results) => results,
        Err(_) => return TypstFilterResult::empty(0, Some(batcher)), // On batch error, include all
    };

    let mut scanned = Vec::new();
    let mut errors = Vec::new();
    let mut draft_count = 0;

    for (path, result) in files.iter().zip(scan_results) {
        match result {
            Ok(scan) => {
                let meta_json = scan.metadata(label);
                let meta: Option<PageMeta> = meta_json
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok());

                // Check if draft
                let is_draft_page = meta.as_ref().map(|m| m.draft).unwrap_or(false);
                if is_draft_page {
                    draft_count += 1;
                    continue;
                }

                // Determine page kind from accessed packages
                let kind = PageKind::from_packages(scan.accessed_packages());

                // Extract internal page links (site-root links only)
                let links: Vec<String> = scan
                    .links()
                    .into_iter()
                    .filter(|link| link.is_site_root())
                    .map(|link| link.dest)
                    .collect();

                // Extract headings
                let headings: Vec<ScannedHeading> = scan
                    .headings()
                    .into_iter()
                    .map(|h| ScannedHeading {
                        level: h.level,
                        text: h.text,
                        supplement: h.supplement,
                    })
                    .collect();

                scanned.push(ScannedPage {
                    path: (*path).clone(),
                    meta,
                    kind,
                    links,
                    headings,
                });
            }
            Err(e) => {
                // Collect error for reporting
                errors.push(((*path).clone(), e));
            }
        }
    }

    TypstFilterResult::new(draft_count, Some(batcher), scanned, errors)
}

/// Check if a Typst scan result indicates a draft page
#[allow(dead_code)]
#[inline]
fn is_draft(scan: &typst_batch::ScanResult, label: &str) -> bool {
    scan.metadata(label)
        .as_ref()
        .and_then(|m| m.get("draft"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// DraftFilter trait implementation (without batcher, for generic use)
impl DraftFilter for Typst {
    type Extra = ();

    fn filter_drafts<'a>(
        files: Vec<&'a PathBuf>,
        root: &'a Path,
        label: &str,
    ) -> FilterResult<'a, Self::Extra> {
        let result = filter_drafts(&files, root, label);
        // Note: batcher is discarded here. Use filter_drafts() directly to get batcher.
        let non_draft_files: Vec<_> = result
            .scanned
            .iter()
            .filter_map(|s| files.iter().find(|f| ***f == s.path).copied())
            .collect();
        FilterResult::new(non_draft_files, result.draft_count)
    }
}
