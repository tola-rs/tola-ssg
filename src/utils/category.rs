//! File category classification for watch mode and incremental builds.
//!
//! This module provides utilities to categorize files by their role in the build process,
//! determining whether changes require full rebuilds or incremental updates.
//!
//! # File Categories
//!
//! | Category   | Rebuild Strategy      | Example Files                |
//! |------------|-----------------------|------------------------------|
//! | Content    | Incremental (single)  | `content/*.typ`              |
//! | Asset      | Incremental (single)  | `assets/images/*`, `*.css`   |
//! | Config     | Full rebuild          | `tola.toml`                  |
//! | Template   | Full rebuild          | `templates/*.typ`            |
//! | Utils      | Full rebuild          | `utils/*.typ`                |
//! | Unknown    | Ignored               | Files outside watched dirs   |
//!
//! # Incremental Build Logic
//!
//! A content file needs rebuilding when:
//! 1. Source `.typ` is newer than output `.html`, OR
//! 2. Any dependency (config/templates/utils) is newer than output

use crate::config::SiteConfig;
use std::{
    env,
    path::{Path, PathBuf},
    time::SystemTime,
};
use walkdir::WalkDir;

/// Category of a changed file, used to determine rebuild strategy in watch mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    /// Content file (.typ) - can be rebuilt individually
    Content,
    /// Asset file - can be copied individually
    Asset,
    /// Site configuration (tola.toml) - requires full site rebuild
    Config,
    /// Template file - requires full site rebuild
    Template,
    /// Shared utility file - requires full site rebuild
    Utils,
    /// File outside watched directories
    Unknown,
}

impl FileCategory {
    /// Returns true if this category requires a full site rebuild
    pub fn requires_full_rebuild(self) -> bool {
        matches!(self, Self::Config | Self::Template | Self::Utils)
    }

    /// Get the short name for this category (used in logs)
    pub fn name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Asset => "assets",
            Self::Config => "config",
            Self::Template => "templates",
            Self::Utils => "utils",
            Self::Unknown => "unknown",
        }
    }

    /// Get a human-readable description for logging
    pub fn description(self, path: &Path) -> String {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        format!("{} ({file_name})", self.name())
    }

    /// Get the directory path for this category from config
    pub fn path(self, config: &SiteConfig) -> Option<PathBuf> {
        match self {
            Self::Content => Some(config.build.content.clone()),
            Self::Asset => Some(config.build.assets.clone()),
            Self::Config => Some(config.config_path.clone()),
            Self::Template => Some(config.build.templates.clone()),
            Self::Utils => Some(config.build.utils.clone()),
            Self::Unknown => None,
        }
    }

    /// Returns true if this category represents a directory (vs a single file)
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Content | Self::Asset | Self::Template | Self::Utils)
    }
}

/// Categorize a file path to determine how changes should be handled.
///
/// Used by the file watcher to decide between incremental and full rebuilds:
/// - `Content`/`Asset`: Process only the changed file
/// - `Config`/`Template`/`Utils`: Trigger full site rebuild
/// - `Unknown`: Ignored
pub fn categorize_path(path: &Path, config: &SiteConfig) -> FileCategory {
    let path = normalize_path(path);

    if path == config.config_path {
        FileCategory::Config
    } else if path.starts_with(&config.build.templates) {
        FileCategory::Template
    } else if path.starts_with(&config.build.utils) {
        FileCategory::Utils
    } else if path.starts_with(&config.build.content) {
        FileCategory::Content
    } else if path.starts_with(&config.build.assets) {
        FileCategory::Asset
    } else {
        FileCategory::Unknown
    }
}

/// Normalize a path to absolute form for reliable comparison.
///
/// Config paths are already canonicalized, so we need to canonicalize
/// incoming paths (e.g., from file watcher) before comparison.
pub fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

/// Get the latest modification time from shared dependencies.
///
/// Shared dependencies include:
/// - `tola.toml` (site configuration)
/// - `templates/` directory (page templates)
/// - `utils/` directory (shared Typst utilities)
///
/// If any of these are newer than a content file's output, that content
/// needs to be recompiled even if the source `.typ` file hasn't changed.
pub fn get_deps_mtime(config: &SiteConfig) -> Option<SystemTime> {
    let deps = [
        Some(config.config_path.clone()),
        Some(config.build.templates.clone()),
        Some(config.build.utils.clone()),
    ];

    deps.iter()
        .filter_map(|p| p.as_ref())
        .filter_map(|p| get_latest_mtime(p))
        .max()
}

/// Get the latest modification time of a file or directory.
///
/// For directories, recursively finds the newest file's mtime.
fn get_latest_mtime(path: &Path) -> Option<SystemTime> {
    if path.is_file() {
        return path.metadata().and_then(|m| m.modified()).ok();
    }

    if path.is_dir() {
        return WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .filter_map(|m| m.modified().ok())
            .max();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ========================================================================
    // FileCategory Tests
    // ========================================================================

    #[test]
    fn test_requires_full_rebuild() {
        // Categories that require full rebuild
        assert!(FileCategory::Config.requires_full_rebuild());
        assert!(FileCategory::Template.requires_full_rebuild());
        assert!(FileCategory::Utils.requires_full_rebuild());

        // Categories that can be rebuilt incrementally
        assert!(!FileCategory::Content.requires_full_rebuild());
        assert!(!FileCategory::Asset.requires_full_rebuild());
        assert!(!FileCategory::Unknown.requires_full_rebuild());
    }

    #[test]
    fn test_category_name() {
        assert_eq!(FileCategory::Content.name(), "content");
        assert_eq!(FileCategory::Asset.name(), "assets");
        assert_eq!(FileCategory::Config.name(), "config");
        assert_eq!(FileCategory::Template.name(), "templates");
        assert_eq!(FileCategory::Utils.name(), "utils");
        assert_eq!(FileCategory::Unknown.name(), "unknown");
    }

    #[test]
    fn test_category_description() {
        let path = Path::new("/some/path/example.typ");
        assert_eq!(
            FileCategory::Content.description(path),
            "content (example.typ)"
        );
        assert_eq!(
            FileCategory::Template.description(path),
            "templates (example.typ)"
        );
    }

    #[test]
    fn test_category_description_no_filename() {
        let path = Path::new("/");
        // Root path has no file_name, should fallback to "unknown"
        assert!(FileCategory::Content.description(path).contains("unknown"));
    }

    #[test]
    fn test_is_directory() {
        // Directory-based categories
        assert!(FileCategory::Content.is_directory());
        assert!(FileCategory::Asset.is_directory());
        assert!(FileCategory::Template.is_directory());
        assert!(FileCategory::Utils.is_directory());

        // Single file or unknown
        assert!(!FileCategory::Config.is_directory());
        assert!(!FileCategory::Unknown.is_directory());
    }

    // ========================================================================
    // normalize_path Tests
    // ========================================================================

    #[test]
    fn test_normalize_path_absolute() {
        let path = Path::new("/absolute/path/file.txt");
        let normalized = normalize_path(path);
        assert!(normalized.is_absolute());
    }

    #[test]
    fn test_normalize_path_relative() {
        let path = Path::new("relative/path/file.txt");
        let normalized = normalize_path(path);
        // Should be converted to absolute (joined with cwd)
        assert!(normalized.is_absolute());
    }
}
