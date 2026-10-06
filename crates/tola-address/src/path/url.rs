//! Decoded site-root paths and the percent encoding at browser boundaries.

use std::borrow::{Borrow, Cow};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::portable::{MAX_PORTABLE_SEGMENT_BYTES, PortablePathError, validate_portable_segment};
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

/// Bytes no URI has literally: the control set, the space, and the other ASCII characters
/// outside the set RFC 3986 allows.
///
/// `?`, `#`, and `%` are URI syntax and stay as written, so a target keeps its separators and an
/// existing escape keeps its spelling; `[` and `]` are gen-delims a URI may hold.
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

/// A decoded site-root path with portable filesystem segments.
///
/// The trailing slash is part of the value: `/x/` names a directory and `/x` names a file, and no
/// lookup may substitute one for the other. Everything after the leading `/` is a decoded filename
/// segment, so a literal `%` or `#` is a name character until [`Self::to_encoded`] renders it.
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
    /// Parse one percent-encoded site-root path, decoding escapes exactly once.
    ///
    /// The trailing slash is preserved. This is a path-only input: `?` and `#` are rejected before
    /// decoding, so a filename with one of them must escape it, and an escaped `/` or `\`
    /// cannot introduce a hierarchy level.
    pub fn parse(encoded_site_path: &str) -> Result<Self, UrlPathError> {
        validate_encoded_path(encoded_site_path)?;
        let decoded = decode_percent_path(encoded_site_path)?;
        Self::validate_decoded(&decoded)?;
        Ok(Self(Arc::from(decoded)))
    }

    /// Construct a path from decoded text, preserving its spelling and case.
    ///
    /// No percent decoding happens and no URL delimiter is interpreted: a literal `%` or `#` is
    /// part of the filename.
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
        if path.chars().any(is_c0_or_c1_control) {
            return Err(UrlPathError::Control);
        }
        if path.starts_with("//") {
            return Err(UrlPathError::Authority);
        }
        let body = path.strip_prefix('/').ok_or(UrlPathError::MissingRoot)?;
        let segments = body.strip_suffix('/').unwrap_or(body);
        if segments.is_empty() {
            return Ok(());
        }
        for (index, segment) in segments.split('/').enumerate() {
            if segment.is_empty() {
                return Err(UrlPathError::EmptySegment);
            }
            if segment == "." || segment == ".." {
                return Err(UrlPathError::DotSegment);
            }
            validate_portable_segment(segment, index == 0).map_err(map_portable_path_error)?;
        }
        Ok(())
    }

    /// Wrap decoded text that already satisfies the segment rules.
    pub(super) fn from_validated_decoded(path: String) -> Self {
        debug_assert!(
            Self::validate_decoded(&path).is_ok(),
            "a derived route is a valid decoded site path"
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
            .filter(|segment| !segment.is_empty())
            .map(super::portable::portable_collision_key)
            .collect()
    }

    /// Percent-encode for a browser URL, including non-ASCII and special characters.
    pub fn to_encoded(&self) -> String {
        let mut encoded = String::with_capacity(self.0.len());
        self.push_encoded(&mut encoded);
        encoded
    }

    /// Append this path's percent-encoded spelling, including its leading `/`.
    ///
    /// A caller filling a buffer it owns writes one fewer whole-string copy than
    /// [`Self::to_encoded`] would.
    pub(crate) fn push_encoded(&self, encoded: &mut String) {
        for (index, segment) in self.0.split('/').enumerate() {
            if index > 0 {
                encoded.push('/');
            }
            for chunk in utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET) {
                encoded.push_str(chunk);
            }
        }
    }
}

/// One site-root browser reference: the decoded route it spelled, plus the suffix that is not
/// identity.
///
/// The route keeps the spelling the reference used, including its trailing slash: `/x/` names a
/// directory and `/x` names a file. The query and fragment never take part in output identity; a
/// resolver looks the route up and only the final document checks a fragment's target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteReference {
    pub route: UrlPath,
    pub query: Option<String>,
    /// Serialized fragment without `#`; its interpretation belongs to the target media type.
    pub fragment: Option<String>,
}

impl SiteReference {
    /// Parse one percent-encoded site-root reference.
    ///
    /// The query and fragment are split before the path is decoded, so an escaped `%3F` or `%23`
    /// stays a filename character.
    pub fn parse(encoded_site_reference: &str) -> Result<Self, UrlPathError> {
        let parts = split_destination(encoded_site_reference);
        Ok(Self {
            route: UrlPath::parse(parts.path)?,
            query: parts.query.map(str::to_owned),
            fragment: parts.fragment.map(str::to_owned),
        })
    }

    /// Decode a fragment for consumers that compare decoded identifiers.
    pub fn decoded_fragment(&self) -> Option<Cow<'_, str>> {
        self.fragment
            .as_deref()
            .map(|fragment| percent_decode_str(fragment).decode_utf8_lossy())
    }

    /// Render the mounted route with its query and serialized fragment intact.
    pub fn to_browser_url(
        &self,
        mount: &crate::SiteUrlMount,
        origin: Option<&crate::SiteOrigin>,
    ) -> String {
        let mut url = crate::browser_url(&self.route, mount, origin);
        if let Some(query) = &self.query {
            url.push('?');
            url.push_str(query);
        }
        if let Some(fragment) = &self.fragment {
            url.push('#');
            url.push_str(fragment);
        }
        url
    }
}

/// A location a browser should request next, derived from the target it just used.
///
/// A location must be a URI: every byte that URI syntax leaves out is percent-encoded, while the
/// bytes it has keep the spelling the request itself used — `?` and `#` stay separators, an
/// escaped target such as `%e6%96%87` stays lowercase, and other non-ASCII text becomes UTF-8
/// escapes.
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

fn is_c0_or_c1_control(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

fn map_portable_path_error(error: PortablePathError) -> UrlPathError {
    match error {
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
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(UrlPathError::InvalidPercentEncoding);
        }
        let high = hex_value(bytes[index + 1]).ok_or(UrlPathError::InvalidPercentEncoding)?;
        let low = hex_value(bytes[index + 2]).ok_or(UrlPathError::InvalidPercentEncoding)?;
        let byte = (high << 4) | low;
        if matches!(byte, b'/' | b'\\') {
            return Err(UrlPathError::EncodedSeparator);
        }
        decoded.push(byte);
        index += 3;
    }

    String::from_utf8(decoded).map_err(|_| UrlPathError::InvalidUtf8)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
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

    fn from_str(encoded_site_path: &str) -> Result<Self, Self::Err> {
        Self::parse(encoded_site_path)
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
        // `Serialize` writes the decoded value, so the wire form is decoded: a literal `%` or
        // `#` round-trips, where `parse` would read a bare `%` as malformed percent encoding.
        let s = String::deserialize(deserializer)?;
        Self::from_decoded(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::url_path;

    #[test]
    fn trailing_slash_is_preserved() {
        for raw in ["/", "/notes/", "/notes", "/notes.txt", "/a/b.html", "/a/b/"] {
            assert_eq!(url_path(raw).as_str(), raw);
            assert_eq!(UrlPath::from_decoded(raw).unwrap().as_str(), raw);
        }
    }

    #[test]
    fn encoded_paths_decode_once() {
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
    fn query_and_fragment_are_refused() {
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
    fn escaped_separators_are_refused() {
        for raw in ["/a%2Fb/", "/a%2fb/", "/a%5Cb/", "/a%5cb/"] {
            assert_eq!(
                UrlPath::parse(raw),
                Err(UrlPathError::EncodedSeparator),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn literal_percent_and_hash_survive() {
        for raw in ["/100%/", "/a#b", "/a%b/c#d"] {
            let decoded = UrlPath::from_decoded(raw).unwrap();
            assert_eq!(decoded.as_str(), raw);
            assert_eq!(url_path(&decoded.to_encoded()), decoded, "{raw:?}");
        }
        assert_eq!(url_path("/100%25/").as_str(), "/100%/");
        assert_eq!(url_path("/a%23b").as_str(), "/a#b");
    }

    #[test]
    fn encoded_and_decoded_paths_refuse_ambiguity() {
        for raw in ["/%2e%2e/outside/", "//host/path", "/a//b/", "/con/"] {
            assert!(UrlPath::parse(raw).is_err(), "accepted {raw:?}");
        }
        for raw in ["/a//b/", "/a/../b", "/con/", "C:/a", "/a\\b"] {
            assert!(UrlPath::from_decoded(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn control_characters_are_refused() {
        for control in (0_u32..=0x1f).chain(0x7f..=0x9f) {
            let character = char::from_u32(control).expect("control range is valid Unicode");
            let raw = format!("/before{character}after/");
            assert!(UrlPath::parse(&raw).is_err(), "accepted U+{control:04X}");
        }
        for raw in [
            "/before%01after/",
            "/before%7Fafter/",
            "/before%C2%80after/",
        ] {
            assert_eq!(UrlPath::parse(raw), Err(UrlPathError::Control));
        }
    }

    #[test]
    fn unicode_encodes_at_the_browser_boundary() {
        let decoded = UrlPath::from_decoded("/中文 hello").unwrap();
        assert_eq!(decoded.as_str(), "/中文 hello");
        assert_eq!(decoded.to_encoded(), "/%E4%B8%AD%E6%96%87%20hello");
    }

    #[test]
    fn nonportable_segments_are_refused() {
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
    fn segment_length_limit_is_enforced() {
        let accepted = format!("/{}/", "a".repeat(MAX_PORTABLE_SEGMENT_BYTES));
        assert!(UrlPath::parse(&accepted).is_ok());

        let rejected = format!("/{}/", "A".repeat(MAX_PORTABLE_SEGMENT_BYTES + 1));
        assert!(matches!(
            UrlPath::parse(&rejected),
            Err(UrlPathError::OutputSegmentTooLong { .. })
        ));
    }

    #[test]
    fn site_reference_spells_the_route() {
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
            assert_eq!(SiteReference::parse(raw).unwrap().route.as_str(), route);
        }
    }

    #[test]
    fn site_reference_splits_query_and_fragment() {
        let reference = SiteReference::parse("/search/?q=rust#res%20ults").unwrap();

        assert_eq!(reference.route.as_str(), "/search/");
        assert_eq!(reference.query.as_deref(), Some("q=rust"));
        assert_eq!(reference.fragment.as_deref(), Some("res%20ults"));
        assert_eq!(reference.decoded_fragment().as_deref(), Some("res ults"));

        let bare = SiteReference::parse("/about.html").unwrap();
        assert_eq!(bare.query, None);
        assert_eq!(bare.fragment, None);
    }

    #[test]
    fn site_reference_reports_path_errors() {
        for raw in ["/%2F/", "/../outside/", "relative", "", "/a%00b"] {
            assert!(SiteReference::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn mounted_reference_preserves_suffixes() {
        let reference = SiteReference::parse("/100%25/?q=1#part%2520x").unwrap();
        let mount = crate::SiteUrlMount::from_base_path("/docs/").unwrap();
        assert_eq!(
            reference.to_browser_url(&mount, None),
            "/docs/100%25/?q=1#part%2520x"
        );
        let origin = crate::SiteOrigin::parse("https://example.test").unwrap();
        assert_eq!(
            reference.to_browser_url(&mount, Some(&origin)),
            "https://example.test/docs/100%25/?q=1#part%2520x"
        );
    }

    #[test]
    fn location_encodes_uri_illegal_bytes() {
        for (target, location) in [
            ("/a b\"c<d>e", "/a%20b%22c%3Cd%3Ee"),
            ("/a\\b^c`d", "/a%5Cb%5Ec%60d"),
            ("/a{b}|c", "/a%7Bb%7D%7Cc"),
            ("/café/?q=naïve#a b", "/caf%C3%A9/?q=na%C3%AFve#a%20b"),
        ] {
            assert_eq!(browser_location(target), location, "{target:?}");
        }
    }

    #[test]
    fn location_preserves_request_spelling() {
        for (target, location) in [
            ("/café/", "/caf%C3%A9/"),
            ("/%e6%96%87/%E6%A1%A3/", "/%e6%96%87/%E6%A1%A3/"),
            ("/café/?q=naïve", "/caf%C3%A9/?q=na%C3%AFve"),
            ("/plain/", "/plain/"),
        ] {
            assert_eq!(browser_location(target), location, "{target:?}");
        }
    }

    #[test]
    fn serde_preserves_the_trailing_slash() {
        for raw in ["/", "/a/b/", "/a/b", "/100%/"] {
            let path = UrlPath::from_decoded(raw).unwrap();
            let json = serde_json::to_string(&path).unwrap();
            let parsed: UrlPath = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, path, "{raw:?}");
        }
        assert!(serde_json::from_str::<UrlPath>("\"/a?b\"").is_err());
    }
}
