//! Path normalization utilities.
//!
//! Provides consistent path handling across the codebase:
//! - `normalize_path` - file system paths (canonicalize + fallback)
//! - `resolve_path` - resolve relative paths with fallback directory

use std::path::{Path, PathBuf};

/// Normalize a file system path to absolute form.
///
/// Tries `canonicalize()` first (resolves symlinks, `.`, `..`).
/// Falls back to:
/// - Return as-is if already absolute
/// - Join with current directory if relative
///
/// # Example
/// ```ignore
/// use tola::utils::path::normalize_path;
/// let abs = normalize_path(Path::new("./content/post.typ"));
/// ```
#[inline]
pub fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
        }
    })
}

/// Resolve a path that may be relative to cwd or a fallback directory.
///
/// Always returns an absolute path.
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
}
