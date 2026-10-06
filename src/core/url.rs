//! URL path type for type-safe URL handling.
//!
//! - Internal representation: Always decoded (human-readable)
//! - Browser boundary: Decode on input, encode on output

use std::borrow::Borrow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// Decoded URL path (internal representation)
///
/// Invariants:
/// - Always decoded (no percent-encoding)
/// - Always starts with `/`
/// - Page URLs end with `/`, asset URLs may not
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UrlPath(Arc<str>);

impl UrlPath {
    /// Create from browser URL (decode percent-encoding, strip query string).
    pub fn from_browser(encoded: &str) -> Self {
        use percent_encoding::percent_decode_str;
        // Strip query string and fragment before decoding (on encoded string)
        let path = encoded.split(['?', '#']).next().unwrap_or(encoded);
        let decoded = percent_decode_str(path)
            .decode_utf8()
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| path.to_string());
        // Use from_page_internal to skip redundant query/fragment stripping
        Self::from_page_internal(&decoded)
    }

    /// Create page URL (with trailing slash). Normalizes leading/trailing slashes.
    /// Strips query string and fragment.
    pub fn from_page(decoded: &str) -> Self {
        let trimmed = decoded.trim();

        // Handle root path specially
        if trimmed.is_empty() || trimmed == "/" {
            return Self(Arc::from("/"));
        }

        // Use url crate to properly strip query and fragment
        let path = Self::strip_query_fragment(trimmed);

        Self::from_page_internal(&path)
    }

    /// Internal: create page URL without stripping query/fragment (already done).
    fn from_page_internal(path: &str) -> Self {
        let trimmed = path.trim();

        // Handle root path specially
        if trimmed.is_empty() || trimmed == "/" {
            return Self(Arc::from("/"));
        }

        // Add leading slash if missing
        let with_leading = if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{}", trimmed)
        };

        // Add trailing slash if missing (for page URLs)
        let normalized = if with_leading.ends_with('/') {
            with_leading
        } else {
            format!("{}/", with_leading)
        };

        Self(Arc::from(normalized))
    }

    /// Strip query string and fragment from a path using url crate.
    fn strip_query_fragment(path: &str) -> String {
        use percent_encoding::percent_decode_str;

        // Use a dummy base URL to parse the path
        static BASE: std::sync::OnceLock<url::Url> = std::sync::OnceLock::new();
        let base = BASE.get_or_init(|| url::Url::parse("http://x").unwrap());

        match base.join(path) {
            Ok(parsed) => {
                // url crate returns percent-encoded path, decode it
                percent_decode_str(parsed.path())
                    .decode_utf8()
                    .map(|s| s.into_owned())
                    .unwrap_or_else(|_| parsed.path().to_string())
            }
            // Fallback to simple split if url parsing fails
            Err(_) => path.split(['?', '#']).next().unwrap_or(path).to_string(),
        }
    }

    /// Create asset URL (no trailing slash normalization).
    pub fn from_asset(decoded: &str) -> Self {
        let trimmed = decoded.trim();

        // Handle empty path
        if trimmed.is_empty() {
            return Self(Arc::from("/"));
        }

        // Add leading slash if missing
        let normalized = if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{}", trimmed)
        };

        Self(Arc::from(normalized))
    }

    /// Get the decoded URL path as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Encode for browser (percent-encode non-ASCII and special characters).
    pub fn to_encoded(&self) -> String {
        self.0
            .split('/')
            .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET).to_string())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Convert to filesystem-safe filename (replaces `/` with `_`).
    pub fn to_safe_filename(&self) -> String {
        self.0.replace('/', "_")
    }

    /// Check if path starts with the given prefix.
    #[inline]
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }

    /// Check if this is a page URL (ends with `/`).
    #[inline]
    pub fn is_page_url(&self) -> bool {
        self.0.ends_with('/')
    }

    /// Check if the URL path is empty (only contains `/`).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty() || self.0.as_ref() == "/"
    }

    /// Get parent URL path.
    ///
    /// `/posts/hello/` -> `/posts/`, `/posts/` -> `/`, `/` -> `None`
    pub fn parent(&self) -> Option<Self> {
        let trimmed = self.0.trim_end_matches('/');
        if trimmed.is_empty() {
            return None;
        }
        match trimmed.rfind('/') {
            Some(0) => Some(Self(Arc::from("/"))),
            Some(idx) => Some(Self(Arc::from(format!("{}/", &trimmed[..idx])))),
            None => Some(Self(Arc::from("/"))),
        }
    }

    /// Compare ignoring trailing slash.
    pub fn matches_ignoring_trailing_slash(&self, other: &str) -> bool {
        let self_trimmed = self.0.trim_end_matches('/');
        let other_trimmed = other.trim_end_matches('/');

        if self_trimmed.is_empty() && other_trimmed.is_empty() {
            return true;
        }
        self_trimmed == other_trimmed
    }

    /// Build canonical URL string from optional site base URL.
    ///
    /// - `Some("https://example.com") + /posts/hello/` -> `https://example.com/posts/hello/`
    /// - `Some("https://example.com/") + /posts/hello/` -> `https://example.com/posts/hello/`
    /// - `Some("https://example.com/blog") + /posts/hello/` -> `https://example.com/blog/posts/hello/`
    /// - `Some("https://example.com/blog") + /blog/posts/hello/` -> `https://example.com/blog/blog/posts/hello/`
    /// - `None + /posts/hello/` -> `/posts/hello/`
    pub fn canonical_url(&self, site_url: Option<&str>) -> String {
        let path = self.to_encoded();
        let Some(site_url) = site_url else {
            return path;
        };

        let Ok(base) = url::Url::parse(site_url) else {
            return format!("{}{}", site_url.trim_end_matches('/'), path);
        };

        let origin = base.origin().ascii_serialization();
        let path = canonical_path(&path, base.path());

        format!("{origin}{path}")
    }

    /// Map URL path to generated HTML output path.
    ///
    /// - `/` -> `{output_dir}/index.html`
    /// - `/posts/hello/` -> `{output_dir}/posts/hello/index.html`
    pub fn output_html_path(&self, output_dir: &Path) -> PathBuf {
        let rel = self.as_str().trim_matches('/');
        if rel.is_empty() {
            output_dir.join("index.html")
        } else {
            output_dir.join(rel).join("index.html")
        }
    }
}

fn canonical_path(path: &str, base_path: &str) -> String {
    let base_path = base_path.trim_matches('/');
    let site_path = path.trim_start_matches('/');

    match (base_path.is_empty(), site_path.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => format!("/{site_path}"),
        (false, true) => format!("/{base_path}/"),
        (false, false) => format!("/{base_path}/{site_path}"),
    }
}

impl std::fmt::Display for UrlPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for UrlPath {
    fn default() -> Self {
        Self::from_page("/")
    }
}

impl AsRef<str> for UrlPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for UrlPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for UrlPath {
    fn from(s: String) -> Self {
        Self::from_page(&s)
    }
}

impl From<&str> for UrlPath {
    fn from(s: &str) -> Self {
        Self::from_page(s)
    }
}

impl PartialEq<str> for UrlPath {
    fn eq(&self, other: &str) -> bool {
        self.0.as_ref() == other
    }
}

impl PartialEq<&str> for UrlPath {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_ref() == *other
    }
}

impl Serialize for UrlPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UrlPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_page(&s))
    }
}

/// URL change notification (permalink changed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlChange {
    /// Previous URL (decoded)
    pub old: UrlPath,
    /// New URL (decoded)
    pub new: UrlPath,
}

impl UrlChange {
    /// Create a new URL change.
    pub fn new(old: impl Into<UrlPath>, new: impl Into<UrlPath>) -> Self {
        Self {
            old: old.into(),
            new: new.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_from_page_cases(cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            assert_eq!(UrlPath::from_page(input).as_str(), *expected, "{input:?}");
        }
    }

    fn assert_from_browser_cases(cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            assert_eq!(
                UrlPath::from_browser(input).as_str(),
                *expected,
                "{input:?}"
            );
        }
    }

    #[test]
    fn test_from_browser_cases() {
        assert_from_browser_cases(&[
            ("/posts/%E4%B8%AD%E6%96%87/", "/posts/中文/"),
            ("/posts/hello%20world/", "/posts/hello world/"),
            ("/posts/%26%3D%3F/", "/posts/&=?/"),
            ("/posts/%FF/", "/posts/%FF/"),
        ]);
    }

    #[test]
    fn test_to_encoded_chinese() {
        let url = UrlPath::from_page("/posts/中文/");
        assert_eq!(url.to_encoded(), "/posts/%E4%B8%AD%E6%96%87/");
    }

    #[test]
    fn test_to_encoded_space() {
        let url = UrlPath::from_page("/posts/hello world/");
        assert_eq!(url.to_encoded(), "/posts/hello%20world/");
    }

    #[test]
    fn test_to_encoded_preserves_path_safe_chars() {
        let url = UrlPath::from_asset("/feed.json");
        assert_eq!(url.to_encoded(), "/feed.json");

        let url = UrlPath::from_asset("/assets/app-1.0_~.js");
        assert_eq!(url.to_encoded(), "/assets/app-1.0_~.js");
    }

    #[test]
    fn test_to_safe_filename() {
        let url = UrlPath::from_page("/posts/hello/");
        assert_eq!(url.to_safe_filename(), "_posts_hello_");
    }

    #[test]
    fn test_starts_with() {
        let url = UrlPath::from_page("/posts/hello/");
        assert!(url.starts_with("/posts"));
        assert!(url.starts_with("/posts/"));
        assert!(!url.starts_with("/about"));
    }

    #[test]
    fn test_is_page_url() {
        assert!(UrlPath::from_page("/posts/hello/").is_page_url());
        assert!(UrlPath::from_page("/").is_page_url());
        assert!(!UrlPath::from_asset("/assets/logo.png").is_page_url());
    }

    #[test]
    fn test_matches_ignoring_trailing_slash() {
        let url = UrlPath::from_page("/posts/hello/");
        assert!(url.matches_ignoring_trailing_slash("/posts/hello"));
        assert!(url.matches_ignoring_trailing_slash("/posts/hello/"));

        let url = UrlPath::from_page("/posts/hello");
        assert!(url.matches_ignoring_trailing_slash("/posts/hello"));
        assert!(url.matches_ignoring_trailing_slash("/posts/hello/"));
    }

    #[test]
    fn test_matches_ignoring_trailing_slash_root() {
        let url = UrlPath::from_page("/");
        assert!(url.matches_ignoring_trailing_slash("/"));
        assert!(url.matches_ignoring_trailing_slash(""));
    }

    #[test]
    fn test_equality() {
        let url1 = UrlPath::from_page("/posts/hello/");
        let url2 = UrlPath::from_page("/posts/hello/");
        let url3 = UrlPath::from_page("/posts/world/");

        assert_eq!(url1, url2);
        assert_ne!(url1, url3);
    }

    #[test]
    fn test_hash() {
        use rustc_hash::FxHashSet;

        let mut set = FxHashSet::default();
        set.insert(UrlPath::from_page("/posts/hello/"));
        set.insert(UrlPath::from_page("/posts/hello/")); // duplicate

        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_serialize_deserialize() {
        let url = UrlPath::from_page("/posts/中文/");
        let json = serde_json::to_string(&url).unwrap();
        assert_eq!(json, r#""/posts/中文/""#);

        let parsed: UrlPath = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, url);
    }

    #[test]
    fn test_parent() {
        // Nested path
        assert_eq!(
            UrlPath::from_page("/posts/hello/").parent(),
            Some(UrlPath::from_page("/posts/"))
        );
        // Single level
        assert_eq!(
            UrlPath::from_page("/posts/").parent(),
            Some(UrlPath::from_page("/"))
        );
        // Root has no parent
        assert_eq!(UrlPath::from_page("/").parent(), None);
        // Deep nesting
        assert_eq!(
            UrlPath::from_page("/a/b/c/").parent(),
            Some(UrlPath::from_page("/a/b/"))
        );
    }

    #[test]
    fn test_trait_string_accessors() {
        let url = UrlPath::from_page("/posts/hello/");
        assert_eq!(format!("{}", url), "/posts/hello/");
        let s: &str = url.as_ref();
        assert_eq!(s, "/posts/hello/");
    }

    #[test]
    fn test_strip_query_fragment_internal() {
        // Verify url crate join behavior
        let base = url::Url::parse("http://x").unwrap();

        // Absolute paths
        assert_eq!(base.join("/blog/post?v=1").unwrap().path(), "/blog/post");
        assert_eq!(
            base.join("/blog/post#section").unwrap().path(),
            "/blog/post"
        );
        assert_eq!(
            base.join("/blog/post?v=1#section").unwrap().path(),
            "/blog/post"
        );

        // Relative paths (become absolute via join)
        assert_eq!(base.join("blog/post?v=1").unwrap().path(), "/blog/post");

        // Edge cases
        assert_eq!(base.join("/").unwrap().path(), "/");
        assert_eq!(base.join("?v=1").unwrap().path(), "/");
        assert_eq!(base.join("#section").unwrap().path(), "/");

        // Chinese characters (should be percent-encoded in path)
        let chinese = base.join("/blog/中文?v=1").unwrap();
        assert_eq!(chinese.path(), "/blog/%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn test_from_page_normalizes_query_and_fragment_cases() {
        assert_from_page_cases(&[
            ("/posts/hello/", "/posts/hello/"),
            ("posts/hello/", "/posts/hello/"),
            ("/posts/hello?v=1", "/posts/hello/"),
            ("/posts/hello#section", "/posts/hello/"),
            ("/posts/hello?v=1#section", "/posts/hello/"),
            ("/posts/中文?v=1", "/posts/中文/"),
        ]);
    }

    #[test]
    fn test_canonical_url() {
        let url = UrlPath::from_page("/posts/hello/");
        assert_eq!(
            url.canonical_url(Some("https://example.com")),
            "https://example.com/posts/hello/"
        );
        assert_eq!(
            url.canonical_url(Some("https://example.com/")),
            "https://example.com/posts/hello/"
        );
        assert_eq!(url.canonical_url(None), "/posts/hello/");
    }

    #[test]
    fn test_canonical_url_with_base_path() {
        assert_eq!(
            UrlPath::from_page("/posts/hello/")
                .canonical_url(Some("https://example.com/docs/blog")),
            "https://example.com/docs/blog/posts/hello/"
        );
        assert_eq!(
            UrlPath::from_page("/docs/blog/posts/hello/")
                .canonical_url(Some("https://example.com/docs/blog")),
            "https://example.com/docs/blog/docs/blog/posts/hello/"
        );
        assert_eq!(
            UrlPath::from_page("/").canonical_url(Some("https://example.com/docs/blog")),
            "https://example.com/docs/blog/"
        );
        assert_eq!(
            UrlPath::from_asset("/docs/blog/feed.json")
                .canonical_url(Some("https://example.com/docs/blog")),
            "https://example.com/docs/blog/docs/blog/feed.json"
        );
    }

    #[test]
    fn test_canonical_url_does_not_deduplicate_matching_prefix() {
        assert_eq!(
            UrlPath::from_page("/blog/post/").canonical_url(Some("https://example.com/blog")),
            "https://example.com/blog/blog/post/"
        );
    }

    #[test]
    fn test_canonical_url_encodes_path_segments() {
        let url = UrlPath::from_page("/posts/中文 path/");
        assert_eq!(
            url.canonical_url(Some("https://example.com")),
            "https://example.com/posts/%E4%B8%AD%E6%96%87%20path/"
        );
    }

    #[test]
    fn test_output_html_path() {
        let output = std::path::Path::new("/tmp/site/public");
        assert_eq!(
            UrlPath::from_page("/").output_html_path(output),
            std::path::PathBuf::from("/tmp/site/public/index.html")
        );
        assert_eq!(
            UrlPath::from_page("/posts/hello/").output_html_path(output),
            std::path::PathBuf::from("/tmp/site/public/posts/hello/index.html")
        );
    }
}
