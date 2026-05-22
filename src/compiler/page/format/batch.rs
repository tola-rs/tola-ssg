//! Batch page pre-scan.

use std::path::{Path, PathBuf};

use crate::{compiler::page::TypstHost, config::SiteConfig, logger};

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
    pub fn report_errors(
        &self,
        max_errors: usize,
        root: &Path,
        extra_hints: bool,
    ) -> anyhow::Result<()> {
        if self.errors.is_empty() {
            return Ok(());
        }

        let total_errors = self.errors.len();

        if crate::core::is_serving() {
            if let Some((path, error)) = self.errors.first() {
                let display_path = path.strip_prefix(root).unwrap_or(path);
                let detail = scan_error_detail(error, max_errors, extra_hints);
                logger::status_error(&display_path.display().to_string(), &detail);
            }
            if total_errors > 1 {
                logger::log(
                    "error",
                    format_args!("... and {} more errors", total_errors - 1),
                );
            }
        } else {
            for (path, error) in self.errors.iter().take(max_errors) {
                let display_path = path.strip_prefix(root).unwrap_or(path);
                logger::log("error", format_args!("{}", display_path.display()));
                logger::text(&scan_error_detail(error, max_errors, extra_hints));
            }

            if total_errors > max_errors {
                logger::text(&format!(
                    "... and {} more errors",
                    total_errors - max_errors
                ));
            }

            logger::blank();
        }

        Err(anyhow::anyhow!(
            "scan failed with {} error(s)",
            total_errors
        ))
    }
}

fn scan_error_detail(
    error: &typst_batch::CompileError,
    max_errors: usize,
    extra_hints: bool,
) -> String {
    let detail = super::super::format_compile_error(error, max_errors).to_string();
    if extra_hints && let Some(hint) = tola_pages_empty_array_hint(error) {
        format!("{detail}\n\n{} {hint}", logger::style_hint("hint:"))
    } else {
        detail
    }
}

fn tola_pages_empty_array_hint(error: &typst_batch::CompileError) -> Option<&'static str> {
    let diagnostics = error.diagnostics()?;
    let pages_empty = diagnostics.errors().any(|diagnostic| {
        diagnostic.message.contains("array is empty")
            && diagnostic.source_lines.iter().any(source_line_calls_pages)
    });

    pages_empty.then_some(
        "The array returned by `pages()` from `@tola/pages` may be empty during scan or after filtering.\nFor `.first()` or similar array access, use `.first(default: none)` and handle the none case explicitly.\nIf this does not match your case, you can ignore this hint.",
    )
}

fn source_line_calls_pages(line: &typst_batch::SourceLine) -> bool {
    let highlighted = line
        .highlight
        .and_then(|(start, end)| line.text.get(start..end));
    highlighted.is_some_and(contains_pages_call) || contains_pages_call(&line.text)
}

fn contains_pages_call(text: &str) -> bool {
    text.contains("pages()")
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

#[cfg(test)]
mod tests {
    #[test]
    fn scan_error_detail_respects_extra_hints_flag() {
        assert!(
            !super::scan_error_detail(
                &typst_batch::CompileError::html_export("array is empty"),
                1,
                false
            )
            .contains("hint:")
        );
    }
}
