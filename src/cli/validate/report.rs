//! Validation report types and formatting.

use std::collections::BTreeMap;
use std::fmt;

use owo_colors::OwoColorize;

use crate::utils::plural_s;

/// A single validation error.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// The link/path that failed.
    pub target: String,
    /// Error reason/message.
    pub reason: String,
}

/// Unified validation report for all error types.
#[derive(Debug, Default)]
pub struct ValidationReport {
    /// Internal link errors (broken page links), grouped by source file.
    pub internal: BTreeMap<String, Vec<ValidationError>>,
    /// Asset errors (missing files), grouped by source file.
    pub assets: BTreeMap<String, Vec<ValidationError>>,
}

impl ValidationReport {
    /// Add an internal link error.
    pub fn add_internal(&mut self, source: String, link: String, reason: String) {
        self.internal
            .entry(source)
            .or_default()
            .push(ValidationError { target: link, reason });
    }

    /// Add an asset error.
    pub fn add_asset(&mut self, source: String, path: String, reason: String) {
        self.assets
            .entry(source)
            .or_default()
            .push(ValidationError { target: path, reason });
    }

    /// Count of files with internal link errors.
    pub fn internal_file_count(&self) -> usize {
        self.internal.len()
    }

    /// Count of files with asset errors.
    pub fn asset_file_count(&self) -> usize {
        self.assets.len()
    }

    /// Total internal link error count.
    pub fn internal_error_count(&self) -> usize {
        self.internal.values().map(|v| v.len()).sum()
    }

    /// Total asset error count.
    pub fn asset_error_count(&self) -> usize {
        self.assets.values().map(|v| v.len()).sum()
    }

    /// Print the full report to stdout (internal -> assets).
    pub fn print(&self) {
        self.print_section("internal links", &self.internal);
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
            format!("({file_count} file{}, {error_count} error{})",
                plural_s(file_count), plural_s(error_count)).dimmed()
        );

        for (path, errs) in errors {
            // File path
            eprintln!("{}{}{}", "[".dimmed(), path.cyan(), "]".dimmed());
            for e in errs {
                eprintln!("{} {}", "→".red(), e.target);
            }
        }
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let internal = self.internal_error_count();
        let assets = self.asset_error_count();
        let total = internal + assets;

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
