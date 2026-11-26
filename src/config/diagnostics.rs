//! Limits for terminal errors and warnings.
//!
//! # Example
//!
//! ```toml
//! [diagnostics]
//! max_errors = 3
//! max_warnings = 3
//! ```

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Limit how many errors and warnings appear in the terminal.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "diagnostics")]
pub(crate) struct DiagnosticsConfig {
    /// Maximum number of errors to print at a time. If omitted, show up to 3. Use `0` to hide errors.
    pub(crate) max_errors: Option<usize>,

    /// Maximum number of warnings to print at a time. If omitted, show up to 3. Use `0` to hide warnings.
    pub(crate) max_warnings: Option<usize>,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            max_errors: Some(3),
            max_warnings: Some(3),
        }
    }
}
