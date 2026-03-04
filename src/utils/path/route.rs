//! URL processing utilities.
//!
//! Provides consistent URL handling across the codebase:
//! - Path normalization (leading slash handling)
//! - Safe filename generation from URLs
//! - Link type detection (external vs internal)
//! - URL relative resolution
//! - User-visible `path_prefix` stripping

use crate::core::UrlPath;

/// Strip leading slash from a URL path
///
/// # Examples
/// ```
/// use tola::utils::url::strip_leading_slash;
/// assert_eq!(strip_leading_slash("/blog/post"), "blog/post");
/// assert_eq!(strip_leading_slash("blog/post"), "blog/post");
/// assert_eq!(strip_leading_slash("/"), "");
/// ```
#[inline]
#[allow(dead_code)] // Reserved for future use
pub fn strip_leading_slash(url: &str) -> &str {
    url.trim_start_matches('/')
}

/// Convert a URL path to a safe filename for caching
///
/// Replaces `/` with `_` to create filesystem-safe names
/// The leading slash is preserved as `_` to distinguish root from `/index`
///
/// # Examples
/// ```
/// use tola::utils::url::url_to_safe_filename;
/// assert_eq!(url_to_safe_filename("/"), "_");
/// assert_eq!(url_to_safe_filename("/index"), "_index");
/// assert_eq!(url_to_safe_filename("/blog/post"), "_blog_post");
/// ```
#[inline]
pub fn url_to_safe_filename(url: &str) -> String {
    url.replace('/', "_")
}

/// Check if a link is external (has a URL scheme like http:, mailto:, etc.)
///
/// A valid scheme must:
/// - Have at least 1 character before the colon
/// - Only contain ASCII alphanumeric or `+`, `-`, `.`
///
/// # Examples
/// ```
/// use tola::utils::url::is_external_link;
/// assert!(is_external_link("https://example.com"));
/// assert!(is_external_link("mailto:user@example.com"));
/// assert!(!is_external_link("/about"));
/// assert!(!is_external_link("./file.txt"));
/// ```
#[inline]
pub fn is_external_link(link: &str) -> bool {
    link.find(':').is_some_and(|pos| {
        pos > 0
            && link[..pos]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// Split a URL into path and fragment parts
///
/// # Returns
/// A tuple of (path, fragment) where fragment is empty string if no `#` found
///
/// # Examples
/// ```
/// use tola::utils::url::split_path_fragment;
/// assert_eq!(split_path_fragment("/about#team"), ("/about", "team"));
/// assert_eq!(split_path_fragment("/about"), ("/about", ""));
/// ```
#[inline]
pub fn split_path_fragment(url: &str) -> (&str, &str) {
    url.split_once('#').unwrap_or((url, ""))
}

/// Resolve a relative URL path from a base page URL.
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

fn normalize_prefix(prefix: &str) -> Option<String> {
    let normalized = prefix.trim_matches('/').replace('\\', "/");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Strip configured `path_prefix` from a URL-like string for user-visible output.
///
/// Only strips when the URL starts with the prefix as a full path segment.
pub fn strip_path_prefix(url: &str, prefix: &str) -> String {
    let normalized = if url.starts_with('/') {
        url.to_string()
    } else {
        format!("/{url}")
    };

    let Some(prefix) = normalize_prefix(prefix) else {
        return normalized;
    };

    let prefix_root = format!("/{prefix}");
    let with_slash = format!("{prefix_root}/");

    if normalized == prefix_root || normalized == with_slash {
        return "/".to_string();
    }

    if let Some(rest) = normalized.strip_prefix(&with_slash) {
        return format!("/{rest}");
    }

    normalized
}

/// Strip `path_prefix` and normalize as a page URL.
pub fn strip_path_prefix_from_page_url(url: &str, prefix: &str) -> String {
    UrlPath::from_page(&strip_path_prefix(url, prefix)).to_string()
}

/// Strip prefixed URL fragments in diagnostic messages.
///
/// This is best-effort text cleanup for user-facing logs.
pub fn strip_path_prefix_in_text(text: &str, prefix: &str) -> String {
    let Some(prefix) = normalize_prefix(prefix) else {
        return text.to_string();
    };

    let marker = format!("/{prefix}/");
    let exact = format!("/{prefix}");

    text.replace(&marker, "/").replace(&exact, "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_leading_slash() {
        assert_eq!(strip_leading_slash("/blog/post"), "blog/post");
        assert_eq!(strip_leading_slash("blog/post"), "blog/post");
        assert_eq!(strip_leading_slash("/"), "");
        assert_eq!(strip_leading_slash(""), "");
    }

    #[test]
    fn test_url_to_safe_filename() {
        assert_eq!(url_to_safe_filename("/"), "_");
        assert_eq!(url_to_safe_filename("/index"), "_index");
        assert_eq!(url_to_safe_filename("/blog"), "_blog");
        assert_eq!(url_to_safe_filename("/blog/post"), "_blog_post");
        // No collision between / and /index
        assert_ne!(url_to_safe_filename("/"), url_to_safe_filename("/index"));
    }

    #[test]
    fn test_is_external_link() {
        assert!(is_external_link("https://example.com"));
        assert!(is_external_link("http://example.com"));
        assert!(is_external_link("mailto:user@example.com"));
        assert!(is_external_link("tel:+1234567890"));
        assert!(!is_external_link("/about"));
        assert!(!is_external_link("./file.txt"));
        assert!(!is_external_link("#section"));
    }

    #[test]
    fn test_split_path_fragment() {
        assert_eq!(split_path_fragment("/about#team"), ("/about", "team"));
        assert_eq!(split_path_fragment("/about"), ("/about", ""));
        assert_eq!(split_path_fragment("#section"), ("", "section"));
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

    #[test]
    fn test_strip_path_prefix() {
        assert_eq!(
            strip_path_prefix(
                "/example-sites/starter/showcase/a/",
                "example-sites/starter"
            ),
            "/showcase/a/"
        );
        assert_eq!(
            strip_path_prefix("/example-sites/starter/", "example-sites/starter"),
            "/"
        );
        assert_eq!(
            strip_path_prefix("/showcase/a/", "example-sites/starter"),
            "/showcase/a/"
        );
    }

    #[test]
    fn test_strip_path_prefix_from_page_url() {
        assert_eq!(
            strip_path_prefix_from_page_url(
                "/example-sites/starter/showcase/current-permalink-direct/",
                "example-sites/starter"
            ),
            "/showcase/current-permalink-direct/"
        );
        assert_eq!(
            strip_path_prefix_from_page_url("/example-sites/starter", "example-sites/starter"),
            "/"
        );
    }
}
