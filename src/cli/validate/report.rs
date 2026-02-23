//! Validation report types and formatting.

use std::collections::BTreeMap;
use std::fmt;

use owo_colors::OwoColorize;

use crate::utils::plural_s;

/// A single validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// The link/path that failed.
    pub target: String,
    /// Error reason/message.
    pub reason: String,
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
        });
    }

    /// Add an asset error.
    pub fn add_asset(&mut self, source: String, path: String, reason: String) {
        self.assets
            .entry(source)
            .or_default()
            .push(ValidationError {
                target: path,
                reason,
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
        eprintln!();

        let file_count = errors.len();
        let error_count: usize = errors.values().map(|v| v.len()).sum();

        // Section header
        eprintln!(
            "{} {}",
            name.red().bold(),
            format!(
                "({file_count} file{}, {error_count} error{})",
                plural_s(file_count),
                plural_s(error_count)
            )
            .dimmed()
        );

        for (path, errs) in errors {
            // File path
            eprintln!("{}{}{}", "[".dimmed(), path.cyan(), "]".dimmed());
            for e in errs {
                if e.reason.is_empty() {
                    eprintln!("{} {}", "→".red(), e.target);
                } else {
                    eprintln!("{} {} {}", "→".red(), e.target, e.reason);
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
            write!(f, "{}", "all checks passed".green())
        } else {
            write!(
                f,
                "{} {} {}",
                "found".dimmed(),
                total.to_string().red().bold(),
                format!("error{}", plural_s(total)).dimmed()
            )
        }
    }
}
