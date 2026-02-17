//! `[build.diagnostics]` section configuration.
//!
//! Controls warning/error display behavior including truncation.
//!
//! # Example
//!
//! ```toml
//! [build.diagnostics]
//! max_errors = 1                  # Max errors to display (default: 1)
//! max_warnings = 20               # Total max warnings
//! max_warnings_per_file = 3       # Max warnings per file
//! max_lines = 100                 # Total max lines
//! max_lines_per_warning = 10      # Max lines per warning
//! ```

use serde::{Deserialize, Serialize};

/// Diagnostics display configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsConfig {
    /// Maximum errors to display per file.
    /// Default is 5 to avoid cascading error spam from syntax errors.
    pub max_errors: usize,

    /// Maximum total warnings to display.
    pub max_warnings: Option<usize>,

    /// Maximum warnings per file.
    pub max_warnings_per_file: Option<usize>,

    /// Maximum total lines across all warnings.
    pub max_lines: Option<usize>,

    /// Maximum lines per individual warning.
    pub max_lines_per_warning: Option<usize>,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            max_errors: 3,
            max_warnings: Some(3),
            max_warnings_per_file: Some(3),
            max_lines: None,
            max_lines_per_warning: None,
        }
    }
}

impl DiagnosticsConfig {
    /// Effective max warnings considering both total and per-file limits.
    /// Given file_count, returns the effective max.
    pub fn effective_max_warnings(&self, file_count: usize) -> Option<usize> {
        match (self.max_warnings, self.max_warnings_per_file) {
            (Some(total), Some(per_file)) => Some(total.min(per_file * file_count)),
            (Some(total), None) => Some(total),
            (None, Some(per_file)) => Some(per_file * file_count),
            (None, None) => None,
        }
    }

    /// Effective max lines considering both total and per-warning limits.
    /// Given warning_count, returns the effective max.
    pub fn effective_max_lines(&self, warning_count: usize) -> Option<usize> {
        match (self.max_lines, self.max_lines_per_warning) {
            (Some(total), Some(per_warn)) => Some(total.min(per_warn * warning_count)),
            (Some(total), None) => Some(total),
            (None, Some(per_warn)) => Some(per_warn * warning_count),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = DiagnosticsConfig::default();
        assert_eq!(config.max_warnings, Some(3));
        assert_eq!(config.max_warnings_per_file, Some(3));
    }

    #[test]
    fn test_effective_max_warnings() {
        // Only total
        let config = DiagnosticsConfig {
            max_warnings: Some(20),
            max_warnings_per_file: None,
            ..Default::default()
        };
        assert_eq!(config.effective_max_warnings(5), Some(20));

        // Only per-file
        let config = DiagnosticsConfig {
            max_warnings: None,
            max_warnings_per_file: Some(3),
            ..Default::default()
        };
        assert_eq!(config.effective_max_warnings(5), Some(15)); // 3 * 5

        // Both: take min
        let config = DiagnosticsConfig {
            max_warnings: Some(10),
            max_warnings_per_file: Some(3),
            ..Default::default()
        };
        assert_eq!(config.effective_max_warnings(5), Some(10)); // min(10, 15)
        assert_eq!(config.effective_max_warnings(2), Some(6)); // min(10, 6)
    }

    #[test]
    fn test_effective_max_lines() {
        let config = DiagnosticsConfig {
            max_lines: Some(100),
            max_lines_per_warning: Some(10),
            ..Default::default()
        };
        assert_eq!(config.effective_max_lines(5), Some(50)); // min(100, 50)
        assert_eq!(config.effective_max_lines(20), Some(100)); // min(100, 200)
    }
}
