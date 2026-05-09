use std::path::PathBuf;

use rustc_hash::FxHashMap;

use crate::config::SiteConfig;
use crate::core::{Priority, UrlPath};

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
    priority: Option<Priority>,
}

impl BatchEntry {
    fn reload(path: impl Into<String>, priority: Option<Priority>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Reload,
            error: None,
            priority,
        }
    }

    fn unchanged(path: impl Into<String>, priority: Option<Priority>) -> Self {
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
        self.priority == Some(Priority::Active)
    }

    fn error_detail(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

fn format_result_counts(
    error_count: usize,
    reload_count: usize,
    unchanged_count: usize,
    warning_count: usize,
) -> String {
    format!(
        "errors: {error_count}, reloads: {reload_count}, unchanged: {unchanged_count}, warnings: {warning_count}"
    )
}

fn format_primary_error_detail(primary_error: &BatchEntry) -> String {
    let path = primary_error.path();
    match primary_error.error_detail() {
        Some(detail) if !detail.is_empty() => format!("first error: {path}\n{detail}"),
        _ => format!("first error: {path}"),
    }
}

/// Aggregates batch results and conflicts for unified output
pub(super) struct BatchLogger {
    results: Vec<BatchEntry>,
    conflicts: FxHashMap<UrlPath, Vec<PathBuf>>,
    permalink_changes: Vec<(PathBuf, UrlPath, UrlPath)>, // (path, old_url, new_url)
    warnings: Vec<String>,
}

impl BatchLogger {
    pub(super) fn new() -> Self {
        Self {
            results: Vec::new(),
            conflicts: FxHashMap::default(),
            permalink_changes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Record a successful reload.
    pub(super) fn push_reload(&mut self, path: impl Into<String>, priority: Option<Priority>) {
        self.results.push(BatchEntry::reload(path, priority));
    }

    /// Record an unchanged result.
    pub(super) fn push_unchanged(&mut self, path: impl Into<String>, priority: Option<Priority>) {
        self.results.push(BatchEntry::unchanged(path, priority));
    }

    /// Record an error.
    pub(super) fn push_error(&mut self, path: impl Into<String>, error: impl Into<String>) {
        self.results.push(BatchEntry::error(path, error));
    }

    pub(super) fn push_warnings<I>(&mut self, warnings: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.warnings.extend(warnings);
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
    pub(super) fn flush(&mut self, config: &SiteConfig) {
        self.output_permalink_changes();
        self.output_conflicts();
        self.output_results(config);
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

    fn output_results(&mut self, config: &SiteConfig) {
        if self.results.is_empty() {
            return;
        }

        let warning_count = self.warnings.len();
        let errors: Vec<_> = self.results.iter().filter(|e| e.is_error()).collect();
        let reloads: Vec<_> = self.results.iter().filter(|e| e.is_reload()).collect();
        let unchanged: Vec<_> = self.results.iter().filter(|e| e.is_unchanged()).collect();

        let primary_reload = reloads.iter().find(|e| e.is_primary()).or(reloads.first());

        if !errors.is_empty() {
            let primary_error = &errors[0];
            let summary =
                format_result_counts(errors.len(), reloads.len(), unchanged.len(), warning_count);
            let detail = format_primary_error_detail(primary_error);
            crate::logger::status_error(&summary, &detail);
        } else {
            if !self.warnings.is_empty() {
                let max = config.build.diagnostics.max_warnings.unwrap_or(usize::MAX);
                for warning in self.warnings.iter().take(max) {
                    eprintln!("{warning}");
                }
                let remaining = self.warnings.len().saturating_sub(max);
                if remaining > 0 {
                    eprintln!("... and {} more warning(s)", remaining);
                }
            }

            if let Some(primary) = primary_reload {
                let other_count = reloads.len() - 1 + unchanged.len();
                let mut msg = match other_count {
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
                if warning_count > 0 {
                    msg.push_str(&format!(", warnings: {warning_count}"));
                }
                crate::logger::status_success(&msg);
            } else if !unchanged.is_empty() {
                let first = unchanged[0].path();
                let mut msg = match unchanged.len() {
                    1 => format!("unchanged: {}", first),
                    n => format!("unchanged: {}, others: {}", first, n - 1),
                };
                if warning_count > 0 {
                    msg.push_str(&format!(", warnings: {warning_count}"));
                }
                crate::logger::status_unchanged(&msg);
            } else if warning_count > 0 {
                crate::logger::status_warning(&format!("warnings: {warning_count}"));
            }
        }

        self.results.clear();
        self.warnings.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchEntry, format_primary_error_detail, format_result_counts};

    #[test]
    fn result_counts_include_all_status_buckets() {
        assert_eq!(
            format_result_counts(2, 3, 4, 5),
            "errors: 2, reloads: 3, unchanged: 4, warnings: 5"
        );
    }

    #[test]
    fn primary_error_detail_includes_path_and_body() {
        let entry = BatchEntry::error("content/index.typ", "error: boom");
        assert_eq!(
            format_primary_error_detail(&entry),
            "first error: content/index.typ\nerror: boom"
        );
    }
}
