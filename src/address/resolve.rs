//! Link resolution types and utilities.
//!
//! This module provides types for resolving links within the address space,
//! including context for resolution and result types.

use std::path::{Path, PathBuf};

use super::Resource;
use crate::core::UrlPath;

// ============================================================================
// Resolve Context & Result
// ============================================================================

/// Context for resolving a link
#[derive(Debug, Clone)]
pub struct ResolveContext<'a> {
    /// Current page's permalink (e.g., /posts/hello/)
    pub current_permalink: &'a UrlPath,
    /// Current page's source file path
    pub source_path: &'a Path,
    /// Colocated assets directory (if any)
    pub colocated_dir: Option<&'a Path>,
    /// HTML attribute containing the link (href, src, etc.)
    pub attr: &'a str,
}

impl ResolveContext<'_> {
    /// Check if this is an asset attribute (src, poster, data).
    pub fn is_asset_attr(&self) -> bool {
        matches!(self.attr, "src" | "poster" | "data")
    }
}

/// Result of resolving a link
#[derive(Debug, Clone)]
pub enum ResolveResult {
    /// Successfully found the target resource.
    Found(Resource),

    /// External link (needs HTTP validation).
    External(String),

    /// Target not found.
    NotFound {
        /// The original link target
        target: String,
        /// Paths that were tried (for diagnostics)
        tried: Vec<String>,
    },

    /// Fragment not found on target page.
    FragmentNotFound {
        /// The page URL
        page: String,
        /// The fragment that wasn't found
        fragment: String,
        /// Available fragments on the page (for suggestions)
        available: Vec<String>,
    },

    /// Warning: link resolves but may have issues.
    Warning {
        /// The resolved URL (if any)
        resolved: Option<String>,
        /// Warning message
        message: String,
    },

    /// Error: link cannot resolve correctly.
    Error {
        /// Error message with suggested fix
        message: String,
    },
}

impl ResolveResult {
    /// Check if this result should block the build.
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::NotFound { .. } | Self::Error { .. })
    }

    /// Check if this result is a warning.
    pub const fn is_warning(&self) -> bool {
        matches!(self, Self::Warning { .. } | Self::FragmentNotFound { .. })
    }

    /// Check if resolution succeeded (Found or External).
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Found(_) | Self::External(_))
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Resolve a relative URL path
///
/// Examples:
/// - base="/posts/hello/", rel="../world/" -> "/posts/world/"
/// - base="/archive/2024/", rel="../../about/" -> "/about/"
pub fn resolve_relative_url(base: &UrlPath, rel: &str) -> UrlPath {
    // Split base into segments (remove empty parts from trailing slashes)
    let mut segments: Vec<&str> = base
        .as_str()
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // Process relative path
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(part),
        }
    }

    // Rebuild URL
    if segments.is_empty() {
        UrlPath::from_page("/")
    } else {
        UrlPath::from_page(&format!("/{}/", segments.join("/")))
    }
}

/// Resolve a relative physical path
pub fn resolve_physical_path(base: &Path, rel: &str) -> PathBuf {
    let clean_rel = rel.trim_start_matches("./");
    let mut result = base.to_path_buf();

    for part in clean_rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                result.pop();
            }
            _ => result.push(part),
        }
    }

    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageRoute;

    #[test]
    fn test_resolve_result_checks() {
        let route = PageRoute {
            source: PathBuf::new(),
            permalink: UrlPath::default(),
            output_file: PathBuf::new(),
            is_index: false,
            is_404: false,
            colocated_dir: None,
            output_dir: PathBuf::new(),
            full_url: String::new(),
            relative: String::new(),
        };
        let found = ResolveResult::Found(Resource::Page { route, title: None });
        assert!(found.is_ok());
        assert!(!found.is_error());
        assert!(!found.is_warning());

        let external = ResolveResult::External("https://example.com".to_string());
        assert!(external.is_ok());

        let not_found = ResolveResult::NotFound {
            target: "/missing/".to_string(),
            tried: vec![],
        };
        assert!(not_found.is_error());
        assert!(!not_found.is_ok());

        let warning = ResolveResult::Warning {
            resolved: Some("/test/".to_string()),
            message: "test".to_string(),
        };
        assert!(warning.is_warning());
        assert!(!warning.is_error());
    }

    #[test]
    fn test_resolve_relative_url() {
        let base = UrlPath::from_page("/posts/hello/");
        assert_eq!(resolve_relative_url(&base, "../world/"), "/posts/world/");

        let base = UrlPath::from_page("/archive/2024/");
        assert_eq!(resolve_relative_url(&base, "../../about/"), "/about/");

        let base = UrlPath::from_page("/a/b/c/");
        assert_eq!(resolve_relative_url(&base, "../../../"), "/");
    }
}
