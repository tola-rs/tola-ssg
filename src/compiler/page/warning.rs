//! Compilation warnings collection.
//!
//! Collects warnings (e.g., unknown font family) during compilation.
//! Call `drain_warnings()` after build to get and clear all warnings.

use parking_lot::Mutex;
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
