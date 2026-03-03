//! Compilation warnings collection.
//!
//! Collects warnings (e.g., unknown font family) during compilation.
//! Call `drain_warnings()` after build to get and clear all warnings.

use parking_lot::Mutex;
use std::path::Path;
use std::sync::LazyLock;
use typst_batch::{DiagnosticInfo, Diagnostics};

/// Global warnings collector for compilation warnings
/// Uses Vec to preserve all warnings (deduplication happens at display time if needed)
static WARNINGS: LazyLock<Mutex<Vec<DiagnosticInfo>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Add warnings from a Diagnostics collection
pub fn collect_warnings(diagnostics: &Diagnostics) {
    if !diagnostics.is_empty() {
        WARNINGS.lock().extend(diagnostics.iter().cloned());
    }
}

/// Drain all collected warnings
///
/// Returns all warnings and clears the collector
/// Should be called after build completes to display warnings
pub fn drain_warnings() -> Diagnostics {
    let items = std::mem::take(&mut *WARNINGS.lock());
    Diagnostics::from_vec(items)
}

/// Get warning source path relative to site root (or `<unknown>`).
pub fn warning_relative_path(warning: &DiagnosticInfo, root: &Path) -> String {
    warning
        .path
        .as_deref()
        .map(|path_str| {
            let path = Path::new(path_str);
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// Format warning with unified prefix:
/// `[warning] <relative-path>`
/// followed by the original warning body.
pub fn format_warning_with_prefix(warning: &DiagnosticInfo, root: &Path) -> String {
    let rel_path = warning_relative_path(warning, root);
    format!("[warning] {rel_path}\n{warning}")
}
