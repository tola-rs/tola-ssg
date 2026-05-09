//! Batch page pre-scan.

use std::path::{Path, PathBuf};

use crate::{compiler::page::TypstHost, config::SiteConfig};

use super::ScannedPage;

/// Result of scanning Typst and Markdown pages before full compilation.
///
/// The scan phase removes draft pages, collects metadata, extracts page links
/// and headings, and keeps the Typst batcher snapshot available for compile.
pub struct PageScanResult<'a> {
    /// Typst batcher for snapshot reuse in compilation.
    pub(super) batcher: Option<super::super::TypstBatcher<'a>>,
    /// Pre-scanned data for all non-draft pages.
    pub scanned: Vec<ScannedPage>,
    /// Total number of draft files filtered out.
    pub drafts_skipped: usize,
    /// Errors encountered during scan phase.
    errors: Vec<(PathBuf, typst_batch::CompileError)>,
}

impl<'a> PageScanResult<'a> {
    /// Snapshot captured during Typst pre-scan for reuse during compilation.
    pub(in crate::compiler::page) fn snapshot(&self) -> Option<super::super::FileSnapshot> {
        self.batcher.as_ref().and_then(|batcher| batcher.snapshot())
    }

    /// Report errors and return an error if any exist.
    pub fn report_errors(&self, max_errors: usize, root: &Path) -> anyhow::Result<()> {
        if self.errors.is_empty() {
            return Ok(());
        }

        let total_errors = self.errors.len();

        if crate::core::is_serving() {
            if let Some((path, error)) = self.errors.first() {
                let display_path = path.strip_prefix(root).unwrap_or(path);
                let detail = super::super::format_compile_error(error, max_errors).to_string();
                crate::logger::status_error(&display_path.display().to_string(), &detail);
            }
            if total_errors > 1 {
                crate::log!("error"; "... and {} more errors", total_errors - 1);
            }
        } else {
            for (path, error) in self.errors.iter().take(max_errors) {
                let display_path = path.strip_prefix(root).unwrap_or(path);
                crate::log!("error"; "{}", display_path.display());
                let err = super::super::format_compile_error(error, max_errors);
                eprintln!("{}", err);
            }

            if total_errors > max_errors {
                eprintln!("... and {} more errors", total_errors - max_errors);
            }
        }

        Err(anyhow::anyhow!(
            "scan failed with {} error(s)",
            total_errors
        ))
    }
}

/// Scan page files from all supported formats.
pub fn scan_pages<'a>(
    config: &'a SiteConfig,
    typst_host: &'a TypstHost,
    typst_files: &[&PathBuf],
    markdown_files: &[&PathBuf],
) -> PageScanResult<'a> {
    let root = config.get_root();
    let label = &config.build.meta.label;

    let typst_result =
        super::super::typst::filter_drafts(typst_files, root, typst_host, label, config);
    let md_result = super::super::markdown::filter_markdown_drafts(markdown_files, root, label);
    let drafts_skipped = typst_result.draft_count + md_result.draft_count;

    PageScanResult {
        batcher: typst_result.batcher,
        scanned: [typst_result.scanned, md_result.scanned].concat(),
        drafts_skipped,
        errors: typst_result.errors,
    }
}
