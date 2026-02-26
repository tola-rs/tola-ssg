use std::path::PathBuf;

use rustc_hash::FxHashMap;

use crate::core::UrlPath;

/// Batch entry status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchStatus {
    Reload,
    Unchanged,
    Error,
}

/// Entry for batch log output
#[derive(Debug, Clone)]
struct BatchEntry {
    path: String,
    status: BatchStatus,
    error: Option<String>,
    priority: Option<crate::core::Priority>,
}

impl BatchEntry {
    fn reload(path: impl Into<String>, priority: Option<crate::core::Priority>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Reload,
            error: None,
            priority,
        }
    }

    fn unchanged(path: impl Into<String>, priority: Option<crate::core::Priority>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Unchanged,
            error: None,
            priority,
        }
    }

    fn error(path: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Error,
            error: Some(error.into()),
            priority: None,
        }
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn is_error(&self) -> bool {
        self.status == BatchStatus::Error
    }

    fn is_unchanged(&self) -> bool {
        self.status == BatchStatus::Unchanged
    }

    fn is_reload(&self) -> bool {
        self.status == BatchStatus::Reload
    }

    fn is_primary(&self) -> bool {
        self.priority == Some(crate::core::Priority::Active)
    }

    fn error_detail(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Aggregates batch results and conflicts for unified output
pub(super) struct BatchLogger {
    results: Vec<BatchEntry>,
    conflicts: FxHashMap<UrlPath, Vec<PathBuf>>,
    permalink_changes: Vec<(PathBuf, UrlPath, UrlPath)>, // (path, old_url, new_url)
}

impl BatchLogger {
    pub(super) fn new() -> Self {
        Self {
            results: Vec::new(),
            conflicts: FxHashMap::default(),
            permalink_changes: Vec::new(),
        }
    }

    /// Record a successful reload.
    pub(super) fn push_reload(
        &mut self,
        path: impl Into<String>,
        priority: Option<crate::core::Priority>,
    ) {
        self.results.push(BatchEntry::reload(path, priority));
    }

    /// Record an unchanged result.
    pub(super) fn push_unchanged(
        &mut self,
        path: impl Into<String>,
        priority: Option<crate::core::Priority>,
    ) {
        self.results.push(BatchEntry::unchanged(path, priority));
    }

    /// Record an error.
    pub(super) fn push_error(&mut self, path: impl Into<String>, error: impl Into<String>) {
        self.results.push(BatchEntry::error(path, error));
    }

    /// Check if there are any errors in the current batch.
    pub(super) fn has_errors(&self) -> bool {
        self.results.iter().any(|e| e.is_error())
    }

    /// Record a permalink conflict.
    pub(super) fn push_conflict(&mut self, url: &UrlPath, source: PathBuf, existing: PathBuf) {
        let sources = self.conflicts.entry(url.clone()).or_default();
        if !sources.contains(&existing) {
            sources.push(existing);
        }
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    /// Record a permalink change.
    pub(super) fn push_permalink_change(
        &mut self,
        path: PathBuf,
        old_url: UrlPath,
        new_url: UrlPath,
    ) {
        self.permalink_changes.push((path, old_url, new_url));
    }

    /// Output all conflicts and results, then clear.
    pub(super) fn flush(&mut self) {
        self.output_permalink_changes();
        self.output_conflicts();
        self.output_results();
    }

    fn output_permalink_changes(&mut self) {
        for (path, old_url, new_url) in &self.permalink_changes {
            crate::log!("permalink"; "{}: \"{}\" -> \"{}\"", path.display(), old_url, new_url);
        }
        self.permalink_changes.clear();
    }

    fn output_conflicts(&mut self) {
        for (url, sources) in &self.conflicts {
            let sources_str = sources
                .iter()
                .map(|p| format!("`{}`", p.display()))
                .collect::<Vec<_>>()
                .join(", ");
            crate::log!("conflict"; "url \"{}\" owned by {}", url, sources_str);
        }
        self.conflicts.clear();
    }

    fn output_results(&mut self) {
        if self.results.is_empty() {
            return;
        }

        let errors: Vec<_> = self.results.iter().filter(|e| e.is_error()).collect();
        let reloads: Vec<_> = self.results.iter().filter(|e| e.is_reload()).collect();
        let unchanged: Vec<_> = self.results.iter().filter(|e| e.is_unchanged()).collect();

        let primary_reload = reloads.iter().find(|e| e.is_primary()).or(reloads.first());

        if !errors.is_empty() {
            let primary_error = &errors[0];
            let detail = primary_error.error_detail().unwrap_or("");
            let summary = format!("compile error in {}", primary_error.path());
            crate::logger::status_error(&summary, detail);
        } else {
            // Show warnings only when no errors (with configured limit)
            let warnings = crate::compiler::drain_warnings();
            if !warnings.is_empty() {
                let max = crate::config::cfg()
                    .build
                    .diagnostics
                    .max_warnings
                    .unwrap_or(usize::MAX);
                let truncated: String = warnings
                    .iter()
                    .take(max)
                    .map(|w| w.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::logger::status_warning(&truncated);
            }

            if let Some(primary) = primary_reload {
                let other_count = reloads.len() - 1 + unchanged.len();
                let msg = match other_count {
                    0 => format!("reload: {}", primary.path()),
                    1 => {
                        let other = reloads
                            .iter()
                            .chain(unchanged.iter())
                            .find(|e| e.path() != primary.path())
                            .map(|e| e.path())
                            .unwrap_or("?");
                        format!("reload: {}, other: {}", primary.path(), other)
                    }
                    n => format!("reload: {}, others: {}", primary.path(), n),
                };
                crate::logger::status_success(&msg);
            } else if !unchanged.is_empty() {
                let first = unchanged[0].path();
                let msg = match unchanged.len() {
                    1 => format!("unchanged: {}", first),
                    n => format!("unchanged: {}, others: {}", first, n - 1),
                };
                crate::logger::status_unchanged(&msg);
            }
        }

        self.results.clear();
    }
}
