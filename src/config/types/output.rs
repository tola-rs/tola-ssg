//! Generated public output paths.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::{ConfigDiagnostics, FieldPath, PathResolver};
use crate::core::UrlPath;

/// Path of a file generated into the public output tree.
///
/// This is not an asset source path. It is interpreted relative to
/// `PathResolver::output_dir()` and claims the corresponding public URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GeneratedOutputPath(PathBuf);

impl GeneratedOutputPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    #[inline]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    /// Slash-separated output path without a leading slash.
    pub fn logical_path(&self) -> String {
        self.0
            .to_string_lossy()
            .replace('\\', "/")
            .trim_matches('/')
            .to_string()
    }

    /// Site-root URL path, before `path_prefix` is applied.
    pub fn url_path(&self) -> UrlPath {
        UrlPath::from_asset(&self.logical_path())
    }

    /// Browser href with `path_prefix` applied.
    pub fn href(&self, paths: PathResolver<'_>) -> String {
        paths.url_for_rel_path(&self.0)
    }

    /// Filesystem path under `PathResolver::output_dir()`.
    pub fn output_path(&self, paths: PathResolver<'_>) -> PathBuf {
        paths.output_dir().join(&self.0)
    }

    pub fn canonical_url(&self, base_url: Option<&str>) -> String {
        self.url_path().canonical_url(base_url)
    }

    pub fn validate(&self, field: FieldPath, diag: &mut ConfigDiagnostics) {
        validate_generated_output_path(self.as_path(), field, None, diag);
    }

    pub fn validate_labeled(
        &self,
        field: FieldPath,
        label: impl Into<String>,
        diag: &mut ConfigDiagnostics,
    ) {
        validate_generated_output_path(self.as_path(), field, Some(label.into()), diag);
    }

    pub fn validate_indexed(
        &self,
        field: FieldPath,
        idx: usize,
        total: usize,
        diag: &mut ConfigDiagnostics,
    ) {
        let label = (total > 1).then(|| format!("[{idx}]"));
        validate_generated_output_path(self.as_path(), field, label, diag);
    }
}

impl fmt::Display for GeneratedOutputPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl AsRef<Path> for GeneratedOutputPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl From<PathBuf> for GeneratedOutputPath {
    fn from(path: PathBuf) -> Self {
        Self::new(path)
    }
}

impl From<&str> for GeneratedOutputPath {
    fn from(path: &str) -> Self {
        Self::new(path)
    }
}

impl From<String> for GeneratedOutputPath {
    fn from(path: String) -> Self {
        Self::new(path)
    }
}

fn validate_generated_output_path(
    path: &Path,
    field: FieldPath,
    label: Option<String>,
    diag: &mut ConfigDiagnostics,
) {
    if path.as_os_str().is_empty() {
        diag.error(field, labeled_message(label, "path must not be empty"));
        return;
    }

    for comp in path.components() {
        let reason = match comp {
            Component::ParentDir => Some("parent directory '..' not allowed"),
            Component::Prefix(_) | Component::RootDir => Some("absolute paths not allowed"),
            _ => None,
        };
        if let Some(reason) = reason {
            diag.error(
                field,
                labeled_message(
                    label.clone(),
                    format!("output '{}': {reason}", path.display()),
                ),
            );
        }
    }
}

fn labeled_message(label: Option<String>, message: impl fmt::Display) -> String {
    match label {
        Some(label) => format!("{label} {message}"),
        None => message.to_string(),
    }
}
