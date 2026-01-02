//! Compile error persistence.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::CACHE_DIR;

/// Error state file name
const ERRORS_FILE: &str = "errors.json";

/// A single persisted compile error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedError {
    pub path: String,
    pub url_path: String,
    pub error: String,
}

impl PersistedError {
    pub fn new(path: impl Into<String>, url_path: impl Into<String>, error: impl Into<String>) -> Self {
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

/// Collection of persisted compile errors.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PersistedErrorState {
    errors: Vec<PersistedError>,
}

impl PersistedErrorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an error, replacing existing error for same path.
    pub fn push(&mut self, error: PersistedError) {
        // Remove existing error for same path
        self.errors.retain(|e| e.path != error.path);
        self.errors.push(error);
    }

    /// Remove errors for a specific file path.
    /// Returns `true` if any errors were removed.
    pub fn clear_for(&mut self, path: &str) -> bool {
        let before = self.errors.len();
        self.errors.retain(|e| e.path != path);
        self.errors.len() < before
    }

    /// Iterate over all errors.
    pub fn iter(&self) -> impl Iterator<Item = &PersistedError> {
        self.errors.iter()
    }

    /// Get first error (for WsActor initialization).
    pub fn first(&self) -> Option<&PersistedError> {
        self.errors.first()
    }

    /// Count of errors.
    pub fn count(&self) -> usize {
        self.errors.len()
    }

    /// Check if empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}



/// Check if file content is the same as new content.
fn file_content_matches(path: &Path, content: &str) -> bool {
    path.exists() && fs::read_to_string(path).is_ok_and(|existing| existing == content)
}

/// Persist compile errors to disk.
///
/// Only writes if the content has actually changed to avoid unnecessary mtime updates.
pub fn persist_errors(state: &PersistedErrorState, root: &Path) -> std::io::Result<()> {
    let cache_dir = root.join(CACHE_DIR);
    let errors_path = cache_dir.join(ERRORS_FILE);

    fs::create_dir_all(&cache_dir)?;

    let json = serde_json::to_string_pretty(state)?;

    if file_content_matches(&errors_path, &json) {
        crate::debug!("persist"; "errors unchanged, skipping write");
        return Ok(());
    }

    fs::write(&errors_path, &json)?;
    crate::debug!("persist"; "saved {} errors", state.count());
    Ok(())
}

/// Restore compile errors from disk.
pub fn restore_errors(root: &Path) -> std::io::Result<PersistedErrorState> {
    let errors_path = root.join(CACHE_DIR).join(ERRORS_FILE);

    if !errors_path.exists() {
        return Ok(PersistedErrorState::new());
    }

    let json = fs::read_to_string(&errors_path)?;
    let state: PersistedErrorState = serde_json::from_str(&json)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    crate::debug!("persist"; "restored {} errors", state.count());
    Ok(state)
}

/// Clear persisted errors.
#[allow(dead_code)]
pub fn clear_errors(root: &Path) -> std::io::Result<()> {
    let errors_path = root.join(CACHE_DIR).join(ERRORS_FILE);
    if errors_path.exists() {
        fs::remove_file(&errors_path)?;
    }
    Ok(())
}
