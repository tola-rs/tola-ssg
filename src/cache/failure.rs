//! Compile diagnostics (errors and warnings) persistence.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::CACHE_DIR;

/// Diagnostics state file name
const DIAGNOSTICS_FILE: &str = "diagnostics.json";

/// A single persisted compile error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedError {
    pub path: String,
    pub url_path: String,
    pub error: String,
}

impl PersistedError {
    pub fn new(
        path: impl Into<String>,
        url_path: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            url_path: url_path.into(),
            error: error.into(),
        }
    }

    /// Get error details as (path, error) tuple.
    pub fn details(&self) -> (&str, &str) {
        (&self.path, &self.error)
    }
}

/// A single persisted compile warning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedWarning {
    pub path: String,
    pub warning: String,
}

impl PersistedWarning {
    pub fn new(path: impl Into<String>, warning: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            warning: warning.into(),
        }
    }
}

/// Collection of persisted compile diagnostics (errors + warnings)
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PersistedDiagnostics {
    errors: Vec<PersistedError>,
    warnings: Vec<PersistedWarning>,
}

impl PersistedDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    // === Error methods ===

    /// Add an error, replacing existing error for same path.
    pub fn push_error(&mut self, error: PersistedError) {
        self.errors.retain(|e| e.path != error.path);
        self.errors.push(error);
    }

    /// Remove errors for a specific file path.
    pub fn clear_errors_for(&mut self, path: &str) -> bool {
        let before = self.errors.len();
        self.errors.retain(|e| e.path != path);
        self.errors.len() < before
    }

    /// Iterate over all errors.
    pub fn errors(&self) -> impl Iterator<Item = &PersistedError> {
        self.errors.iter()
    }

    /// Get first error.
    pub fn first_error(&self) -> Option<&PersistedError> {
        self.errors.first()
    }

    /// Count of errors.
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// Check if has errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    // === Warning methods ===

    /// Add a single warning.
    pub fn push_warning(&mut self, warning: PersistedWarning) {
        self.warnings.push(warning);
    }

    /// Add warnings for a path, replacing existing warnings for same path.
    pub fn set_warnings(&mut self, path: &str, warnings: Vec<String>) {
        self.warnings.retain(|w| w.path != path);
        for warning in warnings {
            self.warnings.push(PersistedWarning::new(path, warning));
        }
    }

    /// Remove warnings for a specific file path.
    pub fn clear_warnings_for(&mut self, path: &str) {
        self.warnings.retain(|w| w.path != path);
    }

    /// Iterate over all warnings.
    pub fn warnings(&self) -> impl Iterator<Item = &PersistedWarning> {
        self.warnings.iter()
    }

    /// Count of warnings.
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// Check if has warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    // === Combined methods ===

    /// Clear all diagnostics for a path.
    pub fn clear_for(&mut self, path: &str) {
        self.clear_errors_for(path);
        self.clear_warnings_for(path);
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }
}

// === Legacy type alias for backward compatibility ===
pub type PersistedErrorState = PersistedDiagnostics;

/// Check if file content is the same as new content
fn file_content_matches(path: &Path, content: &str) -> bool {
    path.exists() && fs::read_to_string(path).is_ok_and(|existing| existing == content)
}

/// Persist compile diagnostics to disk
pub fn persist_diagnostics(state: &PersistedDiagnostics, root: &Path) -> std::io::Result<()> {
    let cache_dir = root.join(CACHE_DIR);
    let path = cache_dir.join(DIAGNOSTICS_FILE);

    fs::create_dir_all(&cache_dir)?;

    let json = serde_json::to_string_pretty(state)?;

    if file_content_matches(&path, &json) {
        crate::debug!("persist"; "diagnostics unchanged, skipping write");
        return Ok(());
    }

    fs::write(&path, &json)?;
    crate::debug!("persist"; "saved {} errors, {} warnings", state.error_count(), state.warning_count());
    Ok(())
}

/// Restore compile diagnostics from disk
pub fn restore_diagnostics(root: &Path) -> std::io::Result<PersistedDiagnostics> {
    let path = root.join(CACHE_DIR).join(DIAGNOSTICS_FILE);

    if !path.exists() {
        return Ok(PersistedDiagnostics::new());
    }

    let json = fs::read_to_string(&path)?;
    let state: PersistedDiagnostics = serde_json::from_str(&json)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    crate::debug!("persist"; "restored {} errors, {} warnings", state.error_count(), state.warning_count());
    Ok(state)
}

// === Legacy function aliases ===

/// Persist compile errors (legacy alias)
pub fn persist_errors(state: &PersistedErrorState, root: &Path) -> std::io::Result<()> {
    persist_diagnostics(state, root)
}

/// Restore compile errors (legacy alias)
pub fn restore_errors(root: &Path) -> std::io::Result<PersistedErrorState> {
    restore_diagnostics(root)
}
