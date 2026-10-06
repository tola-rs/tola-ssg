//! Validation report types and formatting.

use std::collections::BTreeMap;
use std::fmt;

use crate::{logger, utils::plural_s};

/// A single validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// The link/path that failed.
    pub target: String,
    /// Error reason/message.
    pub reason: String,
    /// Optional fix hint.
    pub hint: Option<String>,
}

/// Unified validation report for all error types
#[derive(Debug, Default)]
pub struct ValidationReport {
    /// Page link errors (broken page links), grouped by source file.
    pub pages: BTreeMap<String, Vec<ValidationError>>,
    /// Asset errors (missing files), grouped by source file.
    pub assets: BTreeMap<String, Vec<ValidationError>>,
}

impl ValidationReport {
    /// Add a page link error.
    pub fn add_page(&mut self, source: String, link: String, reason: String) {
        self.pages.entry(source).or_default().push(ValidationError {
            target: link,
            reason,
            hint: None,
        });
    }

    /// Add an asset error.
    pub fn add_asset(&mut self, source: String, path: String, reason: String) {
        self.add_asset_with_hint(source, path, reason, None);
    }

    /// Add an asset error with an optional fix hint.
    pub fn add_asset_with_hint(
        &mut self,
        source: String,
        path: String,
        reason: String,
        hint: Option<String>,
    ) {
        self.assets
            .entry(source)
            .or_default()
            .push(ValidationError {
                target: path,
                reason,
                hint,
            });
    }

    /// Count of files with page link errors.
    pub fn page_file_count(&self) -> usize {
        self.pages.len()
    }

    /// Count of files with asset errors.
    pub fn asset_file_count(&self) -> usize {
        self.assets.len()
    }

    /// Total page link error count.
    pub fn page_error_count(&self) -> usize {
        self.pages.values().map(|v| v.len()).sum()
    }

    /// Total asset error count.
    pub fn asset_error_count(&self) -> usize {
        self.assets.values().map(|v| v.len()).sum()
    }

    /// Print the full report to stdout (pages -> assets).
    pub fn print(&self) {
        self.print_section("pages", &self.pages);
        self.print_section("assets", &self.assets);
    }

    /// Print section with format (target + reason for non-empty reason).
    fn print_section(&self, name: &str, errors: &BTreeMap<String, Vec<ValidationError>>) {
        if errors.is_empty() {
            return;
        }
        logger::blank();

        let file_count = errors.len();
        let error_count: usize = errors.values().map(|v| v.len()).sum();

        // Section header
        logger::text(&format!(
            "{} {}",
            logger::style_error_strong(name),
            logger::style_dim(format!(
                "({file_count} file{}, {error_count} error{})",
                plural_s(file_count),
                plural_s(error_count)
            ))
        ));

        for (path, errs) in errors {
            // File path
            logger::text(&format!(
                "{}{}{}",
                logger::style_dim("["),
                logger::style_path(path),
                logger::style_dim("]")
            ));
            for e in errs {
                if e.reason.is_empty() {
                    logger::text(&format!("{} {}", logger::style_error("→"), e.target));
                } else {
                    logger::text(&format!(
                        "{} {} {}",
                        logger::style_error("→"),
                        e.target,
                        e.reason
                    ));
                }
                if let Some(hint) = &e.hint {
                    logger::text(&format!("  {} {}", logger::style_warning("hint:"), hint));
                }
            }
        }
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pages = self.page_error_count();
        let assets = self.asset_error_count();
        let total = pages + assets;

        if total == 0 {
            write!(f, "{}", logger::style_success("all checks passed"))
        } else {
            write!(
                f,
                "{} {} {}",
                logger::style_dim("found"),
                logger::style_error_strong(total.to_string()),
                logger::style_dim(format!("error{}", plural_s(total)))
            )
        }
    }
}
