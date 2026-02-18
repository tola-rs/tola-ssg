//! Link classification utilities.

use crate::utils::path::route::is_external_link;

/// Syntactic classification of links
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind<'a> {
    /// External link with URL scheme (https://, mailto:, tel:, etc.)
    External(&'a str),
    /// Pure fragment/anchor link (#section). Value is anchor without `#`.
    Fragment(&'a str),
    /// Site-root-relative path (/about, /posts/hello).
    SiteRoot(&'a str),
    /// File-relative path (./image.png, ../other).
    FileRelative(&'a str),
}

impl<'a> LinkKind<'a> {
    /// Parse a link string into its syntactic kind.
    #[inline]
    pub fn parse(link: &'a str) -> Self {
        if is_external_link(link) {
            Self::External(link)
        } else if let Some(anchor) = link.strip_prefix('#') {
            Self::Fragment(anchor)
        } else if let Some(anchor) = link.strip_prefix("./#") {
            // ./#fragment is semantically equivalent to #fragment (current page anchor)
            Self::Fragment(anchor)
        } else if link.starts_with('/') {
            Self::SiteRoot(link)
        } else {
            Self::FileRelative(link)
        }
    }

    /// Check if link is HTTP/HTTPS.
    #[inline]
    pub fn is_http(link: &str) -> bool {
        link.starts_with("http://") || link.starts_with("https://")
    }

    /// Check if link is file-relative (colocated asset candidate).
    #[inline]
    pub fn is_file_relative(link: &str) -> bool {
        !is_external_link(link) && !link.starts_with('#') && !link.starts_with('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_external() {
        assert!(matches!(
            LinkKind::parse("https://example.com"),
            LinkKind::External("https://example.com")
        ));
        assert!(matches!(
            LinkKind::parse("mailto:user@example.com"),
            LinkKind::External("mailto:user@example.com")
        ));
        assert!(matches!(
            LinkKind::parse("tel:+1234567890"),
            LinkKind::External("tel:+1234567890")
        ));
    }

    #[test]
    fn test_parse_fragment() {
        assert!(matches!(
            LinkKind::parse("#section"),
            LinkKind::Fragment("section")
        ));
        assert!(matches!(
            LinkKind::parse("#my-heading"),
            LinkKind::Fragment("my-heading")
        ));
        // Empty fragment
        assert!(matches!(LinkKind::parse("#"), LinkKind::Fragment("")));

        // ./#fragment is equivalent to #fragment
        assert!(matches!(
            LinkKind::parse("./#section"),
            LinkKind::Fragment("section")
        ));
        assert!(matches!(
            LinkKind::parse("./#my-heading"),
            LinkKind::Fragment("my-heading")
        ));
        // Empty ./#
        assert!(matches!(LinkKind::parse("./#"), LinkKind::Fragment("")));
    }

    #[test]
    fn test_parse_site_root() {
        assert!(matches!(
            LinkKind::parse("/about"),
            LinkKind::SiteRoot("/about")
        ));
        assert!(matches!(
            LinkKind::parse("/posts/hello"),
            LinkKind::SiteRoot("/posts/hello")
        ));
        // With fragment
        assert!(matches!(
            LinkKind::parse("/about#team"),
            LinkKind::SiteRoot("/about#team")
        ));
    }

    #[test]
    fn test_parse_file_relative() {
        assert!(matches!(
            LinkKind::parse("./image.png"),
            LinkKind::FileRelative("./image.png")
        ));
        assert!(matches!(
            LinkKind::parse("../other"),
            LinkKind::FileRelative("../other")
        ));
        assert!(matches!(
            LinkKind::parse("image.png"),
            LinkKind::FileRelative("image.png")
        ));
        // With fragment
        assert!(matches!(
            LinkKind::parse("./page#section"),
            LinkKind::FileRelative("./page#section")
        ));
    }

    #[test]
    fn test_is_http() {
        assert!(LinkKind::is_http("http://example.com"));
        assert!(LinkKind::is_http("https://example.com"));
        assert!(!LinkKind::is_http("mailto:user@example.com"));
        assert!(!LinkKind::is_http("/about"));
    }

    #[test]
    fn test_is_file_relative() {
        assert!(LinkKind::is_file_relative("./image.png"));
        assert!(LinkKind::is_file_relative("image.png"));
        assert!(LinkKind::is_file_relative("../other"));
        assert!(!LinkKind::is_file_relative("https://example.com"));
        assert!(!LinkKind::is_file_relative("#section"));
        assert!(!LinkKind::is_file_relative("/about"));
    }
}
