//! Path normalization utilities.
//!
//! Provides consistent path handling across the codebase:
//! - `normalize_path` - file system paths (canonicalize + fallback)
//! - `resolve_path` - resolve relative paths with fallback directory

use std::path::{Path, PathBuf};

use crate::package::TolaPackage;

/// Normalize a file system path to absolute form
///
/// Tries `canonicalize()` first (resolves symlinks, `.`, `..`)
/// Falls back to `absolute()` for non-existent files (e.g., deleted files)
///
/// This ensures consistent path normalization regardless of file existence,
/// which is critical for dependency graph cleanup when files are removed.
///
/// # Example
/// ```ignore
/// use tola::utils::path::normalize_path;
/// let abs = normalize_path(Path::new("./content/post.typ"));
/// // Virtual package sentinels are preserved as-is
/// let sentinel = normalize_path(Path::new("@tola/pages"));
/// assert_eq!(sentinel, PathBuf::from("@tola/pages"));
/// ```
#[inline]
pub fn normalize_path(path: &Path) -> PathBuf {
    // Virtual package sentinels are preserved as-is
    if TolaPackage::from_sentinel(path).is_some() {
        return path.to_path_buf();
    }

    // Prefer canonicalize (resolves symlinks), fall back to absolute for deleted files
    path.canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Normalize a path for prefix/equality comparisons.
///
/// This canonicalizes the nearest existing ancestor before appending the
/// missing suffix. That keeps comparisons stable when a generated output file
/// does not exist yet but its parent source directory does.
#[inline]
pub fn normalize_existing_prefix(path: &Path) -> PathBuf {
    if TolaPackage::from_sentinel(path).is_some() {
        return path.to_path_buf();
    }
    if let Ok(path) = path.canonicalize() {
        return path;
    }

    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        let Some(name) = current.file_name() else {
            break;
        };
        missing.push(name.to_owned());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }

    let mut normalized = normalize_path(current);
    for name in missing.iter().rev() {
        normalized.push(name);
    }
    normalized
}

/// Resolve a path that may be relative to cwd or a fallback directory
///
/// Always returns an absolute path
///
/// Tries in order:
/// 1. If absolute, use as-is
/// 2. If exists relative to cwd, normalize to absolute
/// 3. Otherwise, resolve relative to fallback_dir
///
/// # Example
/// ```ignore
/// use tola::utils::path::resolve_path;
/// // User passes "posts/hello.typ", fallback is content_dir
/// let resolved = resolve_path(Path::new("posts/hello.typ"), content_dir);
/// ```
#[inline]
pub fn resolve_path(path: &Path, fallback_dir: &Path) -> PathBuf {
    // Absolute path: use as-is
    if path.is_absolute() {
        return path.to_path_buf();
    }

    // Try cwd-relative first (handles `content/posts/example.typ`)
    if path.exists() {
        return normalize_path(path);
    }

    // Fall back to fallback_dir-relative (handles `posts/example.typ`)
    normalize_path(&fallback_dir.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(normalized.is_absolute());
    }

    #[test]
    fn test_resolve_path_absolute() {
        let path = Path::new("/absolute/path");
        let resolved = resolve_path(path, Path::new("/fallback"));
        assert_eq!(resolved, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_resolve_path_fallback() {
        // Non-existent relative path should use fallback
        let path = Path::new("nonexistent/path");
        let resolved = resolve_path(path, Path::new("/fallback"));
        assert_eq!(resolved, PathBuf::from("/fallback/nonexistent/path"));
    }

    #[test]
    fn test_normalize_path_virtual() {
        // Virtual paths starting with @ should be preserved as-is
        let sentinel = Path::new("@tola/pages");
        assert_eq!(normalize_path(sentinel), PathBuf::from("@tola/pages"));

        let sentinel2 = Path::new("@tola/site");
        assert_eq!(normalize_path(sentinel2), PathBuf::from("@tola/site"));
    }

    #[test]
    fn test_normalize_existing_prefix_keeps_missing_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let existing = dir.path().join("assets");
        std::fs::create_dir_all(&existing).unwrap();
        let missing = existing.join("site.css");

        assert_eq!(
            normalize_existing_prefix(&missing),
            normalize_path(&existing).join("site.css")
        );
    }
}
