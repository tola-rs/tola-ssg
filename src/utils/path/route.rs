//! URL processing utilities.
//!
//! Provides consistent URL handling across the codebase:
//! - Path normalization (leading slash handling)
//! - Safe filename generation from URLs
//! - Link type detection (external vs internal)

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
}
