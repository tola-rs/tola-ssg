//! `[build.diagnostics]` section configuration.
//!
//! Controls warning/error display behavior.
//!
//! # Example
//!
//! ```toml
//! [build.diagnostics]
//! max_errors = 3                   # Max errors to display (default: 3)
//! max_warnings = 3                 # Max warnings to display (default: 3)
//! ```

use serde::{Deserialize, Serialize};

/// Diagnostics display configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsConfig {
    /// Maximum errors to display (None = unlimited).
    pub max_errors: Option<usize>,

    /// Maximum warnings to display (None = unlimited).
    pub max_warnings: Option<usize>,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            max_errors: Some(3),
            max_warnings: Some(3),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = DiagnosticsConfig::default();
        assert_eq!(config.max_errors, Some(3));
        assert_eq!(config.max_warnings, Some(3));
    }
}
