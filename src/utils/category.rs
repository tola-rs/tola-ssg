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
    /// Dependency file (templates, utils, etc.) - requires rebuilding dependent content
    Deps,
    /// File outside watched directories
    Unknown,
}

impl FileCategory {
    /// Get the short name for this category (used in logs)
    pub const fn name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Asset => "assets",
            Self::Config => "config",
            Self::Deps => "deps",
            Self::Unknown => "unknown",
        }
    }

    /// Get the directory paths for this category from config.
    /// Returns Vec to support multiple deps directories.
    pub fn paths(self, config: &SiteConfig) -> Vec<PathBuf> {
        match self {
            Self::Content => vec![config.build.content.clone()],
            Self::Asset => vec![config.build.assets.clone()],
            Self::Config => vec![config.config_path.clone()],
            Self::Deps => config.build.deps.clone(),
            Self::Unknown => vec![],
        }
    }

    /// Returns true if this category represents a directory (vs a single file)
    pub const fn is_directory(self) -> bool {
        matches!(self, Self::Content | Self::Asset | Self::Deps)
    }
}

/// Categorize a file path to determine how changes should be handled.
///
/// Used by the file watcher to decide between incremental and full rebuilds:
/// - `Content`/`Asset`: Process only the changed file
/// - `Config`/`Deps`: Trigger rebuild of dependent content files
/// - `Unknown`: Ignored
pub fn categorize_path(path: &Path, config: &SiteConfig) -> FileCategory {
    let path = normalize_path(path);

    if path == config.config_path {
        FileCategory::Config
    } else if config.build.deps.iter().any(|dep| path.starts_with(dep)) {
        FileCategory::Deps
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
                .map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
        }
    })
}

/// Get the latest modification time from shared dependencies.
///
/// Shared dependencies include:
/// - `tola.toml` (site configuration)
/// - All directories in `deps` array (templates, utils, etc.)
///
/// If any of these are newer than a content file's output, that content
/// needs to be recompiled even if the source `.typ` file hasn't changed.
pub fn get_deps_mtime(config: &SiteConfig) -> Option<SystemTime> {
    // Config file mtime
    let config_mtime = get_latest_mtime(&config.config_path);

    // All deps directories mtime
    let deps_mtime = config
        .build
        .deps
        .iter()
        .filter_map(|p| get_latest_mtime(p))
        .max();

    [config_mtime, deps_mtime].into_iter().flatten().max()
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
            .filter_map(Result::ok)
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
    fn test_category_name() {
        assert_eq!(FileCategory::Content.name(), "content");
        assert_eq!(FileCategory::Asset.name(), "assets");
        assert_eq!(FileCategory::Config.name(), "config");
        assert_eq!(FileCategory::Template.name(), "templates");
        assert_eq!(FileCategory::Utils.name(), "utils");
        assert_eq!(FileCategory::Unknown.name(), "unknown");
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
