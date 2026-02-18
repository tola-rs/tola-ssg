//! Configuration utility functions.

use std::path::{Path, PathBuf};

/// Extract path component from a URL string
///
/// Uses `url` crate for proper parsing, handling edge cases like:
/// - Port numbers: `https://example.com:8080/path` -> `path`
/// - Auth info: `https://user:pass@example.com/path` -> `path`
/// - Query strings: `https://example.com/path?query` -> `path`
///
/// Returns `None` if the URL is invalid
///
/// # Examples
/// ```ignore
/// extract_url_path("https://example.github.io/my-project/") -> Some("my-project")
/// extract_url_path("https://example.github.io/a/b/c")       -> Some("a/b/c")
/// extract_url_path("https://example.com")                   -> Some("")
/// extract_url_path("https://example.com:8080/path")         -> Some("path")
/// extract_url_path("invalid")                               -> None
/// ```
pub fn extract_url_path(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;

    // Get path and trim leading/trailing slashes
    let path = parsed.path().trim_matches('/');

    Some(path.to_string())
}

/// Find config file by searching upward from current directory
///
/// Starts from cwd and walks up parent directories until finding `config_name`
/// Returns the absolute path to the config file if found
///
/// # Example
/// ```text
/// /home/user/site/content/posts/  ← cwd
/// /home/user/site/tola.toml       ← found!
/// ```
pub fn find_config_file(config_name: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;

    // First check if config_name is an absolute path or exists in cwd
    if config_name.is_absolute() && config_name.exists() {
        return Some(config_name.to_path_buf());
    }

    // Walk up from cwd looking for config file
    let mut current = cwd.as_path();
    loop {
        let candidate = current.join(config_name);
        if candidate.exists() {
            return Some(candidate);
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) => current = parent,
            None => return None, // Reached filesystem root
        }
    }
}

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_url_path() {
        // Standard GitHub Pages subpath
        assert_eq!(
            extract_url_path("https://example.github.io/my-project/"),
            Some("my-project".to_string())
        );

        // Multiple path components
        assert_eq!(
            extract_url_path("https://example.github.io/a/b/c"),
            Some("a/b/c".to_string())
        );

        // Root path (no subpath)
        assert_eq!(extract_url_path("https://example.com"), Some(String::new()));

        // Root path with trailing slash
        assert_eq!(
            extract_url_path("https://example.com/"),
            Some(String::new())
        );

        // HTTP scheme
        assert_eq!(
            extract_url_path("http://localhost/blog/posts"),
            Some("blog/posts".to_string())
        );

        // Invalid URL (no scheme)
        assert_eq!(extract_url_path("invalid-url"), None);
    }

    #[test]
    fn test_extract_url_path_edge_cases() {
        // Port number should be stripped (path extracted correctly)
        assert_eq!(
            extract_url_path("https://example.com:8080/path"),
            Some("path".to_string())
        );

        // Auth info should be stripped
        assert_eq!(
            extract_url_path("https://user:pass@example.com/path"),
            Some("path".to_string())
        );

        // Query string should be excluded from path
        assert_eq!(
            extract_url_path("https://example.com/path?query=1"),
            Some("path".to_string())
        );

        // Fragment should be excluded from path
        assert_eq!(
            extract_url_path("https://example.com/path#section"),
            Some("path".to_string())
        );
    }
}
