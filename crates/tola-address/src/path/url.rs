//! Decoded site-root paths and percent-encoding at browser boundaries.

use std::borrow::{Borrow, Cow};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::portable::{
    MAX_PORTABLE_SEGMENT_BYTES, PortablePathError, is_control_char, validate_portable_segment,
};
use crate::link::split_destination;

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

// `?`, `#`, `%`, `[`, `]` stay unencoded so query/fragment separators and existing escapes remain intact.
const URI_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Decoded site-root path with portable filesystem segments.
///
/// Trailing slashes are significant: `/x/` names a directory and `/x` names a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UrlPath(Arc<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Error)]
pub enum UrlPathError {
    #[error("the URL is empty")]
    Empty,
    #[error("the URL must start with `/`")]
    MissingRoot,
    #[error("the URL must not start with `//`")]
    Authority,
    #[error("the URL must not start or end with whitespace")]
    SurroundingWhitespace,
    #[error("the URL must not contain a query string or fragment")]
    QueryOrFragment,
    #[error("the URL has invalid percent encoding")]
    InvalidPercentEncoding,
    #[error("the URL has percent encoding that is not valid UTF-8")]
    InvalidUtf8,
    #[error("the URL must not percent-encode `/` or `\\`")]
    EncodedSeparator,
    #[error("the URL must use `/` separators")]
    Backslash,
    #[error("the URL must not contain a NUL character")]
    Nul,
    #[error("the URL must not contain control characters")]
    Control,
    #[error("the URL must not contain an empty segment")]
    EmptySegment,
    #[error("the URL must not contain `.` or `..` segments")]
    DotSegment,
    #[error("the URL must not name a filesystem prefix")]
    FilesystemPrefix,
    #[error("the URL contains a character that cannot appear in a published path")]
    FilesystemUnsafeCharacter,
    #[error("a URL segment must not end with a dot or space")]
    FilesystemTrailingDotOrSpace,
    #[error("a URL segment must not be a Windows device name")]
    WindowsDeviceName,
    #[error("a URL segment uses {byte_len} bytes, over the {max}-byte limit")]
    OutputSegmentTooLong { byte_len: usize, max: usize },
}

impl UrlPath {
    /// Parse a percent-encoded site-root path, decoding escapes exactly once.
    pub fn parse(raw: &str) -> Result<Self, UrlPathError> {
        validate_encoded_path(raw)?;
        let decoded = decode_percent_path(raw)?;
        Self::validate_decoded(&decoded)?;
        Ok(Self(Arc::from(decoded)))
    }

    /// Construct a path from decoded text without percent-decoding (`%` and `#` stay literal).
    pub fn from_decoded(path: &str) -> Result<Self, UrlPathError> {
        if path.trim() != path {
            return Err(UrlPathError::SurroundingWhitespace);
        }
        Self::validate_decoded(path)?;
        Ok(Self(Arc::from(path)))
    }

    fn validate_decoded(path: &str) -> Result<(), UrlPathError> {
        if path.contains('\\') {
            return Err(UrlPathError::Backslash);
        }
        if path.contains('\0') {
            return Err(UrlPathError::Nul);
        }
        if path.chars().any(is_control_char) {
            return Err(UrlPathError::Control);
        }
        if path.starts_with("//") {
            return Err(UrlPathError::Authority);
        }
        let body = path.strip_prefix('/').ok_or(UrlPathError::MissingRoot)?;
        let segs = body.strip_suffix('/').unwrap_or(body);
        if segs.is_empty() {
            return Ok(());
        }
        for (idx, seg) in segs.split('/').enumerate() {
            if seg.is_empty() {
                return Err(UrlPathError::EmptySegment);
            }
            if seg == "." || seg == ".." {
                return Err(UrlPathError::DotSegment);
            }
            validate_portable_segment(seg, idx == 0).map_err(map_portable_path_error)?;
        }
        Ok(())
    }

    pub(super) fn from_validated_decoded(path: String) -> Self {
        debug_assert!(
            Self::validate_decoded(&path).is_ok(),
            "derived route must be a valid decoded site path"
        );
        Self(Arc::from(path))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Portable collision key for each non-empty path segment.
    pub fn url_collision_key(&self) -> Vec<String> {
        self.as_str()
            .trim_matches('/')
            .split('/')
            .filter(|seg| !seg.is_empty())
            .map(super::portable::portable_collision_key)
            .collect()
    }

    /// Percent-encode for a browser URL.
    pub fn to_encoded(&self) -> String {
        let mut buf = String::with_capacity(self.0.len());
        self.push_encoded(&mut buf);
        buf
    }

    /// Append this path's percent-encoded spelling, including its leading `/`.
    pub(crate) fn push_encoded(&self, buf: &mut String) {
        for (idx, seg) in self.0.split('/').enumerate() {
            if idx > 0 {
                buf.push('/');
            }
            for chunk in utf8_percent_encode(seg, PATH_SEGMENT_ENCODE_SET) {
                buf.push_str(chunk);
            }
        }
    }
}

/// Site-root browser reference: decoded route plus optional query and serialized fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRef {
    pub route: UrlPath,
    pub query: Option<String>,
    /// Serialized fragment without `#`.
    pub fragment: Option<String>,
}

impl SiteRef {
    /// Parse a percent-encoded site-root reference, splitting query/fragment before decoding the path.
    pub fn parse(raw: &str) -> Result<Self, UrlPathError> {
        let parts = split_destination(raw);
        Ok(Self {
            route: UrlPath::parse(parts.path)?,
            query: parts.query.map(str::to_owned),
            fragment: parts.fragment.map(str::to_owned),
        })
    }

    /// Decode the fragment for comparing decoded identifiers.
    pub fn decoded_fragment(&self) -> Option<Cow<'_, str>> {
        self.fragment
            .as_deref()
            .map(|frag| percent_decode_str(frag).decode_utf8_lossy())
    }

    /// Render the mounted route with its query and serialized fragment intact.
    pub fn to_browser_url(
        &self,
        mount: &crate::SiteUrlMount,
        origin: Option<&crate::SiteOrigin>,
    ) -> String {
        let mut url = crate::browser_url(&self.route, mount, origin);
        if let Some(q) = &self.query {
            url.push('?');
            url.push_str(q);
        }
        if let Some(frag) = &self.fragment {
            url.push('#');
            url.push_str(frag);
        }
        url
    }
}

/// Percent-encode URI-illegal bytes in `target` while keeping `?`, `#`, and existing `%` escapes.
pub fn browser_location(target: &str) -> String {
    utf8_percent_encode(target, URI_ENCODE_SET).to_string()
}

fn validate_encoded_path(raw: &str) -> Result<(), UrlPathError> {
    if raw.is_empty() {
        return Err(UrlPathError::Empty);
    }
    if raw.trim() != raw {
        return Err(UrlPathError::SurroundingWhitespace);
    }
    if !raw.starts_with('/') {
        return Err(UrlPathError::MissingRoot);
    }
    if raw.starts_with("//") {
        return Err(UrlPathError::Authority);
    }
    if raw.contains(['?', '#']) {
        return Err(UrlPathError::QueryOrFragment);
    }
    Ok(())
}

fn map_portable_path_error(err: PortablePathError) -> UrlPathError {
    match err {
        PortablePathError::FilesystemPrefix => UrlPathError::FilesystemPrefix,
        PortablePathError::UnsafeCharacter => UrlPathError::FilesystemUnsafeCharacter,
        PortablePathError::TrailingDotOrSpace => UrlPathError::FilesystemTrailingDotOrSpace,
        PortablePathError::WindowsDeviceName => UrlPathError::WindowsDeviceName,
        PortablePathError::SegmentTooLong { byte_len } => UrlPathError::OutputSegmentTooLong {
            byte_len,
            max: MAX_PORTABLE_SEGMENT_BYTES,
        },
    }
}

fn decode_percent_path(raw: &str) -> Result<String, UrlPathError> {
    let bytes = raw.as_bytes();
    let mut buf = Vec::with_capacity(bytes.len());
    let mut idx = 0;

    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            buf.push(bytes[idx]);
            idx += 1;
            continue;
        }
        if idx + 2 >= bytes.len() {
            return Err(UrlPathError::InvalidPercentEncoding);
        }
        let hi = hex_val(bytes[idx + 1]).ok_or(UrlPathError::InvalidPercentEncoding)?;
        let lo = hex_val(bytes[idx + 2]).ok_or(UrlPathError::InvalidPercentEncoding)?;
        let b = (hi << 4) | lo;
        if matches!(b, b'/' | b'\\') {
            return Err(UrlPathError::EncodedSeparator);
        }
        buf.push(b);
        idx += 3;
    }

    String::from_utf8(buf).map_err(|_| UrlPathError::InvalidUtf8)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for UrlPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for UrlPath {
    fn default() -> Self {
        Self::from_validated_decoded("/".to_owned())
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

impl FromStr for UrlPath {
    type Err = UrlPathError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
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
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for UrlPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Wire form is decoded (`Serialize` emits `self.0`), so literal `%`/`#` round-trip.
        let s = String::deserialize(d)?;
        Self::from_decoded(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_path(raw: &str) -> UrlPath {
        UrlPath::parse(raw).unwrap()
    }

    #[test]
    fn trailing_slash() {
        for raw in ["/", "/notes/", "/notes", "/notes.txt", "/a/b.html", "/a/b/"] {
            assert_eq!(url_path(raw).as_str(), raw);
            assert_eq!(UrlPath::from_decoded(raw).unwrap().as_str(), raw);
        }
    }

    #[test]
    fn percent_decode() {
        for (raw, expected) in [
            ("/posts/%E4%B8%AD%E6%96%87/", "/posts/中文/"),
            ("/posts/hello%20world/", "/posts/hello world/"),
            ("/posts/%26%3D/", "/posts/&=/"),
            ("/about%2Ehtml", "/about.html"),
        ] {
            assert_eq!(url_path(raw).as_str(), expected, "{raw:?}");
        }
        assert_eq!(
            UrlPath::parse("/posts/%FF/"),
            Err(UrlPathError::InvalidUtf8)
        );
        assert_eq!(
            UrlPath::parse("/%0/"),
            Err(UrlPathError::InvalidPercentEncoding)
        );
    }

    #[test]
    fn invalid_urls() {
        for raw in [
            "/posts/hello?v=1",
            "/posts/hello#section",
            "/posts/hello?v=1#section",
            "posts/hello/",
            "//host/path",
            "",
        ] {
            assert!(UrlPath::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn literal_percent_hash() {
        for raw in ["/100%/", "/a#b", "/a%b/c#d"] {
            let decoded = UrlPath::from_decoded(raw).unwrap();
            assert_eq!(decoded.as_str(), raw);
            assert_eq!(url_path(&decoded.to_encoded()), decoded, "{raw:?}");
        }
        assert_eq!(url_path("/100%25/").as_str(), "/100%/");
        assert_eq!(url_path("/a%23b").as_str(), "/a#b");
    }

    #[test]
    fn dot_and_separator_errors() {
        for raw in ["/%2e%2e/outside/", "//host/path", "/a//b/", "/con/"] {
            assert!(UrlPath::parse(raw).is_err(), "accepted {raw:?}");
        }
        for raw in ["/a//b/", "/a/../b", "/con/", "C:/a", "/a\\b"] {
            assert!(UrlPath::from_decoded(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn unicode_encode() {
        let decoded = UrlPath::from_decoded("/中文 hello").unwrap();
        assert_eq!(decoded.as_str(), "/中文 hello");
        assert_eq!(decoded.to_encoded(), "/%E4%B8%AD%E6%96%87%20hello");
    }

    #[test]
    fn nonportable_segments() {
        for raw in [
            "/con/",
            "/AUX.txt/",
            "/lpt9/",
            "/trail./",
            "/trail%20/",
            "/x%3Cy/",
            "/x%7Cy/",
            "/x%3Fy/",
            "/x%22y/",
        ] {
            assert!(UrlPath::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn segment_length_limit() {
        let ok = format!("/{}/", "a".repeat(MAX_PORTABLE_SEGMENT_BYTES));
        assert!(UrlPath::parse(&ok).is_ok());

        let too_long = format!("/{}/", "A".repeat(MAX_PORTABLE_SEGMENT_BYTES + 1));
        assert!(matches!(
            UrlPath::parse(&too_long),
            Err(UrlPathError::OutputSegmentTooLong { .. })
        ));
    }

    #[test]
    fn site_ref_route() {
        for (raw, route) in [
            ("/", "/"),
            ("/notes", "/notes"),
            ("/notes/", "/notes/"),
            ("/notes.html", "/notes.html"),
            ("/deep/index.html", "/deep/index.html"),
            ("/posts/%E4%B8%AD%E6%96%87/", "/posts/中文/"),
            ("/100%25", "/100%"),
            ("/a%23b/", "/a#b/"),
        ] {
            assert_eq!(SiteRef::parse(raw).unwrap().route.as_str(), route);
        }
    }

    #[test]
    fn site_ref_parts() {
        let site_ref = SiteRef::parse("/search/?q=rust#res%20ults").unwrap();
        assert_eq!(site_ref.route.as_str(), "/search/");
        assert_eq!(site_ref.query.as_deref(), Some("q=rust"));
        assert_eq!(site_ref.fragment.as_deref(), Some("res%20ults"));
        assert_eq!(site_ref.decoded_fragment().as_deref(), Some("res ults"));

        let bare = SiteRef::parse("/about.html").unwrap();
        assert_eq!(bare.query, None);
        assert_eq!(bare.fragment, None);
    }

    #[test]
    fn site_ref_errors() {
        for raw in ["/%2F/", "/../outside/", "relative", "", "/a%00b"] {
            assert!(SiteRef::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn site_ref_browser_url() {
        let site_ref = SiteRef::parse("/100%25/?q=1#part%2520x").unwrap();
        let mount = crate::SiteUrlMount::from_base_path("/docs/").unwrap();
        assert_eq!(
            site_ref.to_browser_url(&mount, None),
            "/docs/100%25/?q=1#part%2520x"
        );
        let origin = crate::SiteOrigin::parse("https://example.test").unwrap();
        assert_eq!(
            site_ref.to_browser_url(&mount, Some(&origin)),
            "https://example.test/docs/100%25/?q=1#part%2520x"
        );
    }

    #[test]
    fn browser_location_encode() {
        for (target, expected) in [
            ("/a b\"c<d>e", "/a%20b%22c%3Cd%3Ee"),
            ("/a\\b^c`d", "/a%5Cb%5Ec%60d"),
            ("/a{b}|c", "/a%7Bb%7D%7Cc"),
            ("/café/?q=naïve#a b", "/caf%C3%A9/?q=na%C3%AFve#a%20b"),
        ] {
            assert_eq!(browser_location(target), expected, "{target:?}");
        }
    }
}
