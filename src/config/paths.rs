//! Centralized path resolution for consistent URL and output path generation.
//!
//! This module provides a single source of truth for all path operations,
//! eliminating manual `path_prefix` handling throughout the codebase.
//!
//! # Architecture
//!
//! ```text
//! SiteConfig
//!     │
//!     └── paths() → PathResolver
//!                       │
//!                       ├── output_root()        → /abs/path/public
//!                       ├── output_dir()         → /abs/path/public/prefix
//!                       ├── url_for_filename()   → /prefix/filename
//!                       └── url_for_path()       → /prefix/path/to/file
//! ```
//!
//! # Usage
//!
//! ```ignore
//! let paths = config.paths();
//!
//! // Get output directory for content files
//! let output = paths.output_dir();
//!
//! // Generate URL for a file
//! let url = paths.url_for_filename("styles.css");
//! // → "/prefix/styles.css" (or "/styles.css" if no prefix)
//! ```

use std::path::{Path, PathBuf};

/// Centralized path resolver for consistent URL and output path generation.
///
/// Provides a unified API for all path operations, ensuring `path_prefix` is
/// correctly applied everywhere without manual handling.
#[derive(Debug, Clone, Copy)]
pub struct PathResolver<'a> {
    /// Output root directory (without path_prefix)
    output: &'a Path,
    /// Path prefix for subdirectory deployment
    prefix: &'a Path,
}

impl<'a> PathResolver<'a> {
    /// Create a new PathResolver from config paths.
    #[inline]
    pub const fn new(output: &'a Path, prefix: &'a Path) -> Self {
        Self { output, prefix }
    }

    /// Raw output directory (without path_prefix).
    ///
    /// Used for:
    /// - Git repository initialization
    /// - Top-level files like `.gitignore`, `.ignore`
    #[inline]
    pub const fn output_root(&self) -> &Path {
        self.output
    }

    /// Content output directory (with path_prefix).
    ///
    /// Where HTML pages, assets, and generated files are placed.
    /// Example: `/path/to/public/my-project/`
    #[inline]
    pub fn output_dir(&self) -> PathBuf {
        self.output.join(self.prefix)
    }

    /// Check if path_prefix is set (non-empty).
    #[inline]
    pub fn has_prefix(&self) -> bool {
        !self.prefix.as_os_str().is_empty()
    }

    /// Get the path prefix.
    #[inline]
    pub const fn prefix(&self) -> &Path {
        self.prefix
    }

    /// Generate URL path for a filename in the output directory.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // With prefix "my-project":
    /// paths.url_for_filename("styles.css") → "/my-project/styles.css"
    ///
    /// // Without prefix:
    /// paths.url_for_filename("styles.css") → "/styles.css"
    /// ```
    pub fn url_for_filename(&self, filename: &str) -> String {
        if self.has_prefix() {
            format!("/{}/{}", self.prefix.display(), filename)
        } else {
            format!("/{filename}")
        }
    }

    /// Generate URL path for a relative path in the output directory.
    ///
    /// Similar to `url_for_filename` but accepts a path with subdirectories.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // With prefix "my-project":
    /// paths.url_for_rel_path("css/app.css") → "/my-project/css/app.css"
    /// ```
    pub fn url_for_rel_path<P: AsRef<Path>>(&self, rel_path: P) -> String {
        let joined = self.prefix.join(rel_path);
        let path_str = joined.to_string_lossy().replace('\\', "/");
        format!("/{path_str}")
    }

    /// Generate URL path from an absolute file path.
    ///
    /// Strips the output root and returns the URL path.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Path: /home/user/public/my-project/css/app.css
    /// // Output root: /home/user/public
    /// // Result: /my-project/css/app.css
    /// ```
    pub fn url_for_path(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.output).ok()?;
        let path_str = rel.to_string_lossy().replace('\\', "/");
        Some(if path_str.starts_with('/') {
            path_str.to_string()
        } else {
            format!("/{path_str}")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_root() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("blog"));
        assert_eq!(paths.output_root(), Path::new("/public"));
    }

    #[test]
    fn test_output_dir_with_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("blog"));
        assert_eq!(paths.output_dir(), PathBuf::from("/public/blog"));
    }

    #[test]
    fn test_output_dir_without_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new(""));
        assert_eq!(paths.output_dir(), PathBuf::from("/public"));
    }

    #[test]
    fn test_has_prefix() {
        let with = PathResolver::new(Path::new("/public"), Path::new("blog"));
        let without = PathResolver::new(Path::new("/public"), Path::new(""));

        assert!(with.has_prefix());
        assert!(!without.has_prefix());
    }

    #[test]
    fn test_url_for_filename_with_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("my-project"));
        assert_eq!(paths.url_for_filename("styles.css"), "/my-project/styles.css");
    }

    #[test]
    fn test_url_for_filename_without_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new(""));
        assert_eq!(paths.url_for_filename("styles.css"), "/styles.css");
    }

    #[test]
    fn test_url_for_rel_path_with_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("blog"));
        assert_eq!(paths.url_for_rel_path("css/app.css"), "/blog/css/app.css");
    }

    #[test]
    fn test_url_for_rel_path_nested_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("sites/blog"));
        assert_eq!(paths.url_for_rel_path("img/logo.png"), "/sites/blog/img/logo.png");
    }

    #[test]
    fn test_url_for_path() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("blog"));
        let file_path = Path::new("/public/blog/posts/hello/index.html");
        assert_eq!(
            paths.url_for_path(file_path),
            Some("/blog/posts/hello/index.html".to_string())
        );
    }

    #[test]
    fn test_url_for_path_not_in_output() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("blog"));
        let file_path = Path::new("/other/path/file.html");
        assert_eq!(paths.url_for_path(file_path), None);
    }
}
