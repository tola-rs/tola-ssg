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
//!     └── paths() -> PathResolver
//!                       │
//!                       ├── output_root()        -> /abs/path/public
//!                       ├── output_dir()         -> /abs/path/public/prefix
//!                       ├── url_for_filename()   -> /prefix/filename
//!                       └── url_for_path()       -> /prefix/path/to/file
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
//! // -> "/prefix/styles.css" (or "/styles.css" if no prefix)
//! ```

use std::path::{Path, PathBuf};

/// Centralized path resolver for consistent URL and output path generation
///
/// Provides a unified API for all path operations, ensuring `path_prefix` is
/// correctly applied everywhere without manual handling
#[derive(Debug, Clone, Copy)]
pub struct PathResolver<'a> {
    /// Output root directory (without path_prefix)
    output: &'a Path,
    /// Path prefix for subdirectory deployment
    prefix: &'a Path,
}

impl<'a> PathResolver<'a> {
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
    #[allow(dead_code)] // Reserved API
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

    #[inline]
    pub fn has_prefix(&self) -> bool {
        !self.prefix.as_os_str().is_empty()
    }

    #[inline]
    #[allow(dead_code)] // Reserved API
    pub const fn prefix(&self) -> &Path {
        self.prefix
    }

    /// Generate URL path for a filename in the output directory.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // With prefix "my-project":
    /// paths.url_for_filename("styles.css") -> "/my-project/styles.css"
    ///
    /// // Without prefix:
    /// paths.url_for_filename("styles.css") -> "/styles.css"
    /// ```
    pub fn url_for_filename(&self, filename: &str) -> String {
        self.url_for_rel_path(filename)
    }

    /// Generate URL path for a relative path in the output directory.
    ///
    /// Similar to `url_for_filename` but accepts a path with subdirectories.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // With prefix "my-project":
    /// paths.url_for_rel_path("css/app.css") -> "/my-project/css/app.css"
    /// ```
    pub fn url_for_rel_path<P: AsRef<Path>>(&self, rel_path: P) -> String {
        let path_str = join_url_paths(&[path_to_url(self.prefix), path_to_url(rel_path.as_ref())]);
        format!("/{path_str}")
    }

    /// Generate browser URL path for a user-authored site-root path.
    ///
    /// The input path is always relative to the site root. If `path_prefix`
    /// is configured, it is always prepended; no prefix de-duplication is
    /// attempted.
    pub fn url_for_site_path<P: AsRef<Path>>(&self, path: P) -> String {
        self.url_for_rel_path(path)
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
    #[allow(dead_code)] // Reserved API
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

fn path_to_url(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

fn join_url_paths(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_building_cases() {
        let with_prefix = PathResolver::new(Path::new("/public"), Path::new("my-project"));
        assert_eq!(
            with_prefix.url_for_filename("styles.css"),
            "/my-project/styles.css"
        );

        let without_prefix = PathResolver::new(Path::new("/public"), Path::new(""));
        assert_eq!(without_prefix.url_for_filename("styles.css"), "/styles.css");

        let blog = PathResolver::new(Path::new("/public"), Path::new("blog"));
        assert_eq!(blog.url_for_rel_path("css/app.css"), "/blog/css/app.css");

        let nested = PathResolver::new(Path::new("/public"), Path::new("sites/blog"));
        assert_eq!(
            nested.url_for_rel_path("img/logo.png"),
            "/sites/blog/img/logo.png"
        );
    }

    #[test]
    fn url_for_rel_path_always_resolves_under_output_prefix() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("blog"));

        assert_eq!(paths.url_for_rel_path("feed.xml"), "/blog/feed.xml");
        assert_eq!(paths.url_for_rel_path("/feed.xml"), "/blog/feed.xml");
        assert_eq!(
            paths.url_for_rel_path("blog/feed.xml"),
            "/blog/blog/feed.xml"
        );
    }

    #[test]
    fn url_for_site_path_treats_input_as_site_root_relative() {
        let paths = PathResolver::new(Path::new("/public"), Path::new("docs/blog"));

        assert_eq!(
            paths.url_for_site_path("posts/hello"),
            "/docs/blog/posts/hello"
        );
        assert_eq!(
            paths.url_for_site_path("docs/blog/posts/hello"),
            "/docs/blog/docs/blog/posts/hello"
        );
        assert_eq!(
            paths.url_for_site_path("docs/blogger"),
            "/docs/blog/docs/blogger"
        );
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
