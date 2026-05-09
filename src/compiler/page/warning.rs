//! Compilation warnings collection.
//!
//! Warning collection is owned by the caller instead of hidden global state.

use parking_lot::Mutex;
use std::path::Path;
use typst_batch::{DiagnosticInfo, Diagnostics};

/// Compilation warning collector.
///
/// Uses `Vec` to preserve all warnings; display-time code owns truncation.
#[derive(Default)]
pub struct WarningCollector {
    items: Mutex<Vec<DiagnosticInfo>>,
}

impl WarningCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add warnings from a Diagnostics collection.
    pub fn collect(&self, diagnostics: &Diagnostics) {
        if !diagnostics.is_empty() {
            self.items.lock().extend(diagnostics.iter().cloned());
        }
    }

    /// Drain all collected warnings.
    pub fn drain(&self) -> Diagnostics {
        let items = std::mem::take(&mut *self.items.lock());
        Diagnostics::from_vec(items)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.items.lock().len()
    }
}

/// Get warning source path relative to site root (or `<unknown>`).
fn warning_relative_path(warning: &DiagnosticInfo, root: &Path) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collector_drains_owned_warnings() {
        let collector = WarningCollector::new();
        let diagnostics = Diagnostics::from_vec(vec![DiagnosticInfo {
            severity: typst_batch::DiagnosticSeverity::Warning,
            message: "first warning".to_string(),
            path: None,
            line: None,
            column: None,
            source_lines: Vec::new(),
            hints: Vec::new(),
            traces: Vec::new(),
        }]);

        collector.collect(&diagnostics);
        assert_eq!(collector.len(), 1);

        let drained = collector.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(collector.len(), 0);
    }
}
