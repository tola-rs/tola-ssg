//! `[build.assets]` section configuration.
//!
//! Handles static assets configuration with two modes:
//! - **nested**: Directories copied with structure preserved
//! - **flatten**: Individual files copied to output root
//!
//! # Example
//!
//! ```toml
//! [build.assets]
//! nested = [
//!     "assets",                              # assets/ → output/assets/
//!     { dir = "vendor/static", as = "lib" }, # vendor/static/ → output/lib/
//! ]
//! flatten = [
//!     "assets/CNAME",                        # → output/CNAME
//!     { file = "icons/fav.ico", as = "favicon.ico" },
//! ]
//! ```

use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

use macros::Config;
use serde::{Deserialize, Serialize};

use crate::config::{ConfigDiagnostics, FieldPath};

// ============================================================================
// Output Name Tracker (Validation Helper)
// ============================================================================

/// Tracks output names to detect conflicts during validation.
struct OutputNameTracker<'a> {
    seen: FxHashMap<&'a str, (&'static str, &'a Path)>,
}

impl<'a> OutputNameTracker<'a> {
    fn new() -> Self {
        Self {
            seen: FxHashMap::default(),
        }
    }

    /// Check for conflict and insert. Reports error if conflict found.
    fn check_and_insert(
        &mut self,
        name: &'a str,
        kind: &'static str,
        path: &'a Path,
        idx: usize,
        field: FieldPath,
        diag: &mut ConfigDiagnostics,
    ) {
        if let Some((prev_kind, prev_path)) = self.seen.get(name) {
            diag.error(
                field,
                format!(
                    "[{idx}] output conflict: {kind} '{}' and {prev_kind} '{}' both output to '/{name}'",
                    path.display(),
                    prev_path.display(),
                ),
            );
        } else {
            self.seen.insert(name, (kind, path));
        }
    }
}

// ============================================================================
// Main Config
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.assets")]
pub struct AssetsConfig {
    /// Nested directories (preserve structure).
    /// Each directory is copied to `output/{basename}/`.
    /// Files inside can be referenced as `/{basename}/path/to/file`.
    /// Examples:
    /// - `"assets"` → `/assets/xxx`
    /// - `"assets/styles"` → `/styles/xxx`
    /// - `{ dir = "vendor", as = "lib" }` → `/lib/xxx`
    pub nested: Vec<NestedEntry>,

    /// Flatten files (copy to output root).
    /// Each file is copied directly to `output/{basename}`.
    /// Examples:
    /// - `"assets/CNAME"` → `/CNAME`
    /// - `{ file = "icons/fav.ico", as = "favicon.ico" }` → `/favicon.ico`
    pub flatten: Vec<FlattenEntry>,
}

impl Default for AssetsConfig {
    fn default() -> Self {
        Self {
            nested: vec![NestedEntry::Simple("assets".into())],
            flatten: vec![],
        }
    }
}

impl AssetsConfig {
    /// Get all nested source directories.
    pub fn nested_sources(&self) -> impl Iterator<Item = &Path> {
        self.nested.iter().map(|e| e.source())
    }

    /// Get all flatten source files.
    #[allow(dead_code)]
    pub fn flatten_sources(&self) -> impl Iterator<Item = &Path> {
        self.flatten.iter().map(|e| e.source())
    }

    /// Check if a flatten entry would output as "CNAME".
    #[allow(dead_code)]
    pub fn has_cname_in_flatten(&self) -> bool {
        self.flatten.iter().any(|e| e.output_name() == "CNAME")
    }

    /// Check if source path is in any nested directory or is a flatten file.
    pub fn contains_source(&self, source: &Path) -> bool {
        self.nested.iter().any(|e| source.starts_with(e.source()))
            || self.flatten.iter().any(|e| source == e.source())
    }

    /// Find which nested entry contains the source path.
    #[allow(dead_code)]
    pub fn find_nested_for(&self, source: &Path) -> Option<&NestedEntry> {
        self.nested.iter().find(|e| source.starts_with(e.source()))
    }

    /// Normalize all paths relative to root directory.
    pub fn normalize(&mut self, root: &Path) {
        for entry in &mut self.nested {
            entry.normalize(root);
        }
        for entry in &mut self.flatten {
            entry.normalize(root);
        }
    }

    /// Check if a source path is a flatten file.
    ///
    /// Used by `scan_global_assets` to skip files that should only
    /// be output to the flatten location (output root).
    pub fn is_flatten(&self, source: &Path) -> bool {
        self.flatten.iter().any(|e| e.source() == source)
    }

    // ========================================================================
    // Validation (Pre-normalization)
    // ========================================================================

    /// Validate path safety before normalization.
    ///
    /// MUST be called before `normalize()` - after normalization all paths
    /// become absolute (joined with root), making this check impossible.
    pub fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        let nested_count = self.nested.len();
        let flatten_count = self.flatten.len();

        self.nested.iter().enumerate().for_each(|(i, e)| {
            Self::validate_path_safety(e.source(), i, nested_count, Self::FIELDS.nested, diag)
        });

        self.flatten.iter().enumerate().for_each(|(i, e)| {
            Self::validate_path_safety(e.source(), i, flatten_count, Self::FIELDS.flatten, diag)
        });
    }

    /// Check a single path for unsafe components (`.."` or absolute).
    fn validate_path_safety(
        path: &Path,
        idx: usize,
        total: usize,
        field: FieldPath,
        diag: &mut ConfigDiagnostics,
    ) {
        use std::path::Component;

        for comp in path.components() {
            let msg = match comp {
                Component::ParentDir => Some("parent directory '..' not allowed"),
                Component::Prefix(_) | Component::RootDir => Some("absolute paths not allowed"),
                _ => None,
            };
            if let Some(reason) = msg {
                // Only show index if there are multiple entries
                let prefix = if total > 1 {
                    format!("[{idx}] ")
                } else {
                    String::new()
                };
                diag.error(
                    field,
                    format!("{prefix}path '{}': {reason}", path.display()),
                );
            }
        }
    }

    // ========================================================================
    // Validation (Post-normalization)
    // ========================================================================

    /// Validate assets configuration after paths are normalized.
    ///
    /// Checks:
    /// - Output name conflicts between entries
    /// - Type correctness (directories vs files)
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        let mut outputs = OutputNameTracker::new();

        for (i, entry) in self.nested.iter().enumerate() {
            Self::validate_nested_entry(entry, i, &mut outputs, diag);
        }

        for (i, entry) in self.flatten.iter().enumerate() {
            Self::validate_flatten_entry(entry, i, &mut outputs, diag);
        }
    }

    fn validate_nested_entry<'a>(
        entry: &'a NestedEntry,
        idx: usize,
        outputs: &mut OutputNameTracker<'a>,
        diag: &mut ConfigDiagnostics,
    ) {
        let path = entry.source();

        // Must be a directory if it exists
        if path.exists() && !path.is_dir() {
            diag.error(
                Self::FIELDS.nested,
                format!("[{idx}] '{}' must be a directory", path.display()),
            );
        }

        // Check output name conflict
        outputs.check_and_insert(
            entry.output_name(),
            "nested",
            path,
            idx,
            Self::FIELDS.nested,
            diag,
        );
    }

    fn validate_flatten_entry<'a>(
        entry: &'a FlattenEntry,
        idx: usize,
        outputs: &mut OutputNameTracker<'a>,
        diag: &mut ConfigDiagnostics,
    ) {
        let path = entry.source();

        // Must be a file if it exists
        if path.exists() && !path.is_file() {
            diag.error(
                Self::FIELDS.flatten,
                format!("[{idx}] '{}' must be a file", path.display()),
            );
        }

        // Check output name conflict
        outputs.check_and_insert(
            entry.output_name(),
            "flatten",
            path,
            idx,
            Self::FIELDS.flatten,
            diag,
        );
    }
}

// ============================================================================
// Nested Entry
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NestedEntry {
    /// Simple path string.
    Simple(PathBuf),
    /// Full format with optional rename.
    Full {
        /// Source directory path (relative to site root).
        dir: PathBuf,
        /// Output directory name (defaults to dir's basename).
        #[serde(rename = "as")]
        output_as: Option<String>,
    },
}

impl NestedEntry {
    /// Get source directory path.
    pub fn source(&self) -> &Path {
        match self {
            Self::Simple(p) => p,
            Self::Full { dir, .. } => dir,
        }
    }

    /// Get output directory name.
    pub fn output_name(&self) -> &str {
        match self {
            Self::Simple(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or("assets"),
            Self::Full { dir, output_as } => output_as
                .as_deref()
                .unwrap_or_else(|| dir.file_name().and_then(|n| n.to_str()).unwrap_or("assets")),
        }
    }

    /// Create a simple entry.
    #[cfg(test)]
    pub fn simple(path: impl Into<PathBuf>) -> Self {
        Self::Simple(path.into())
    }

    /// Create a full entry with rename.
    #[cfg(test)]
    pub fn with_as(dir: impl Into<PathBuf>, output_as: impl Into<String>) -> Self {
        Self::Full {
            dir: dir.into(),
            output_as: Some(output_as.into()),
        }
    }

    /// Normalize source path relative to root directory.
    pub fn normalize(&mut self, root: &Path) {
        match self {
            Self::Simple(p) => {
                *p = crate::utils::path::normalize_path(&root.join(&*p));
            }
            Self::Full { dir, .. } => {
                *dir = crate::utils::path::normalize_path(&root.join(&*dir));
            }
        }
    }
}

// ============================================================================
// Flatten Entry
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlattenEntry {
    /// Simple path string.
    Simple(PathBuf),
    /// Full format with optional rename.
    Full {
        /// Source file path (relative to site root).
        file: PathBuf,
        /// Output file name (defaults to file's basename).
        #[serde(rename = "as")]
        output_as: Option<String>,
    },
}

impl FlattenEntry {
    /// Get source file path.
    pub fn source(&self) -> &Path {
        match self {
            Self::Simple(p) => p,
            Self::Full { file, .. } => file,
        }
    }

    /// Get output file name.
    pub fn output_name(&self) -> &str {
        match self {
            Self::Simple(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or("unknown"),
            Self::Full { file, output_as } => output_as.as_deref().unwrap_or_else(|| {
                file.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
            }),
        }
    }

    /// Create a simple entry.
    #[cfg(test)]
    pub fn simple(path: impl Into<PathBuf>) -> Self {
        Self::Simple(path.into())
    }

    /// Create a full entry with rename.
    #[cfg(test)]
    pub fn with_as(file: impl Into<PathBuf>, output_as: impl Into<String>) -> Self {
        Self::Full {
            file: file.into(),
            output_as: Some(output_as.into()),
        }
    }

    /// Normalize source path relative to root directory.
    pub fn normalize(&mut self, root: &Path) {
        match self {
            Self::Simple(p) => {
                *p = crate::utils::path::normalize_path(&root.join(&*p));
            }
            Self::Full { file, .. } => {
                *file = crate::utils::path::normalize_path(&root.join(&*file));
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nested_entry_simple() {
        let entry = NestedEntry::simple("assets");
        assert_eq!(entry.source(), Path::new("assets"));
        assert_eq!(entry.output_name(), "assets");
    }

    #[test]
    fn test_nested_entry_with_as() {
        let entry = NestedEntry::with_as("vendor/static", "lib");
        assert_eq!(entry.source(), Path::new("vendor/static"));
        assert_eq!(entry.output_name(), "lib");
    }

    #[test]
    fn test_flatten_entry_simple() {
        let entry = FlattenEntry::simple("assets/CNAME");
        assert_eq!(entry.source(), Path::new("assets/CNAME"));
        assert_eq!(entry.output_name(), "CNAME");
    }

    #[test]
    fn test_flatten_entry_with_as() {
        let entry = FlattenEntry::with_as("icons/fav.ico", "favicon.ico");
        assert_eq!(entry.source(), Path::new("icons/fav.ico"));
        assert_eq!(entry.output_name(), "favicon.ico");
    }

    #[test]
    fn test_has_cname_in_flatten() {
        let config: AssetsConfig = toml::from_str(r#"flatten = ["assets/CNAME"]"#).unwrap();
        assert!(config.has_cname_in_flatten());

        let config2: AssetsConfig = toml::from_str(r#"flatten = ["assets/robots.txt"]"#).unwrap();
        assert!(!config2.has_cname_in_flatten());
    }

    #[test]
    fn test_full_config() {
        let toml = r#"
nested = ["assets", { dir = "vendor", as = "lib" }]
flatten = ["CNAME", { file = "fav.ico", as = "favicon.ico" }]
"#;
        let config: AssetsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.nested.len(), 2);
        assert_eq!(config.flatten.len(), 2);
        assert_eq!(config.nested[1].output_name(), "lib");
        assert_eq!(config.flatten[1].output_name(), "favicon.ico");
    }

    #[test]
    fn test_find_nested_for() {
        let config: AssetsConfig =
            toml::from_str(r#"nested = ["assets", { dir = "vendor", as = "lib" }]"#).unwrap();

        let entry = config.find_nested_for(Path::new("assets/images/logo.png"));
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().output_name(), "assets");

        let entry2 = config.find_nested_for(Path::new("vendor/js/app.js"));
        assert!(entry2.is_some());
        assert_eq!(entry2.unwrap().output_name(), "lib");

        let entry3 = config.find_nested_for(Path::new("other/file.txt"));
        assert!(entry3.is_none());
    }

    #[test]
    fn test_is_flatten() {
        let toml = r#"
flatten = [
    "assets/CNAME",
    { file = "assets/logo.png", as = "logo.png" },
]
"#;
        let config: AssetsConfig = toml::from_str(toml).unwrap();

        // Flatten files
        assert!(config.is_flatten(Path::new("assets/CNAME")));
        assert!(config.is_flatten(Path::new("assets/logo.png")));

        // Not a flatten file
        assert!(!config.is_flatten(Path::new("assets/other.txt")));
    }
}
