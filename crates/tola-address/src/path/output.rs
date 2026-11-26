//! Portable logical identities below a site output root.

use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

use super::portable::{is_control_char, validate_portable_segment};
use super::{PortablePathError, UrlPath, portable_collision_key};

/// Segment reserved at the top of every published output root for system assets.
pub const RESERVED_ROOT: &str = "_tola";

/// Portable, non-empty decoded relative file path below the site output root (never ends with `/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputPath(Arc<str>);

impl Serialize for OutputPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OutputPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(|err| de::Error::custom(err.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutputPathError {
    #[error("output path must be non-empty and must not end with `/`")]
    Empty,
    #[error("the output path must be relative to the site output root")]
    Absolute,
    #[error("output path must not contain empty components")]
    EmptyComponent,
    #[error("output path must not contain `.` or `..` components")]
    DotComponent,
    #[error("output path must use `/` separators")]
    Backslash,
    #[error("output path must not contain NUL or control characters")]
    Control,
    #[error(transparent)]
    NonPortable(#[from] PortablePathError),
}

impl OutputPath {
    /// Parse a decoded relative file path without percent-decoding or query/fragment handling.
    pub fn parse(raw: &str) -> Result<Self, OutputPathError> {
        if raw.contains('\\') {
            return Err(OutputPathError::Backslash);
        }
        if raw.chars().any(is_control_char) {
            return Err(OutputPathError::Control);
        }
        if raw.starts_with('/') {
            return Err(OutputPathError::Absolute);
        }
        if raw.is_empty() || raw.ends_with('/') {
            return Err(OutputPathError::Empty);
        }

        for (idx, seg) in raw.split('/').enumerate() {
            validate_segment(seg, idx == 0)?;
        }

        Ok(Self(Arc::from(raw)))
    }

    /// Logical output file corresponding to a decoded site-root route (`/` -> `index.html`).
    pub fn from_route(route: &UrlPath) -> Self {
        let rel = &route.as_str()[1..];
        if rel.is_empty() {
            Self(Arc::from("index.html"))
        } else if rel.ends_with('/') {
            Self(Arc::from(format!("{rel}index.html")))
        } else {
            Self(Arc::from(rel))
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Append a validated descendant without reparsing either path.
    pub fn join(&self, child: &Self) -> Self {
        Self(Arc::from(format!("{}/{}", self.as_str(), child.as_str())))
    }

    pub fn portable_key(&self) -> Vec<String> {
        self.as_str()
            .split('/')
            .map(portable_collision_key)
            .collect()
    }

    pub fn is_reserved_for_non_system_output(&self) -> bool {
        portable_key_is_reserved(&self.portable_key())
    }
}

/// Decoded site-root route where `output` is reachable (`x/index.html` -> `/x/`).
pub fn route_for_output(output: &OutputPath) -> UrlPath {
    asset_directory_index_alias(output).unwrap_or_else(|| asset_url_from_output(output))
}

/// Exact file URL where `output` is served (`x/index.html` -> `/x/index.html`).
pub fn asset_url_from_output(output: &OutputPath) -> UrlPath {
    UrlPath::from_validated_decoded(format!("/{}", output.as_str()))
}

/// Output file named by a site-root file URL, rejecting directory routes (`/` or `/x/`).
pub fn asset_output_from_url(url: &UrlPath) -> Result<OutputPath, OutputPathError> {
    OutputPath::parse(url.as_str().trim_start_matches('/'))
}

/// Directory route aliasing an `index.html` output (`index.html` -> `/`, `x/index.html` -> `/x/`).
pub fn asset_directory_index_alias(output: &OutputPath) -> Option<UrlPath> {
    let route = match output.as_str() {
        "index.html" => "/".to_owned(),
        path => format!("/{}/", path.strip_suffix("/index.html")?),
    };
    Some(UrlPath::from_validated_decoded(route))
}

/// Return whether a portable collision key falls under [`RESERVED_ROOT`].
pub fn portable_key_is_reserved(key: &[String]) -> bool {
    key.first().is_some_and(|seg| seg == RESERVED_ROOT)
}

impl fmt::Display for OutputPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Borrow<str> for OutputPath {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

fn validate_segment(seg: &str, first: bool) -> Result<(), OutputPathError> {
    if seg.is_empty() {
        return Err(OutputPathError::EmptyComponent);
    }
    if matches!(seg, "." | "..") {
        return Err(OutputPathError::DotComponent);
    }
    validate_portable_segment(seg, first)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_paths() {
        assert_eq!(
            OutputPath::parse("/tags/rust/index.html"),
            Err(OutputPathError::Absolute)
        );
        for path in [
            "",
            "a/",
            "a//b",
            "a/../b",
            "a\\b",
            "a?b",
            "C:/a",
            "a/NUL.txt",
            "a/trail.",
        ] {
            assert!(OutputPath::parse(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn route_directory_index() {
        for (route, output) in [
            ("/", "index.html"),
            ("/x/", "x/index.html"),
            ("/x", "x"),
            ("/x.pdf", "x.pdf"),
            ("/x/index.html", "x/index.html"),
        ] {
            assert_eq!(
                OutputPath::from_route(&UrlPath::parse(route).unwrap()).as_str(),
                output
            );
        }
    }

    #[test]
    fn route_output_roundtrip() {
        for route in ["/", "/x/", "/x", "/a/b.pdf", "/100%25/"] {
            let route = UrlPath::parse(route).unwrap();
            assert_eq!(route_for_output(&OutputPath::from_route(&route)), route);
        }
        let exact = UrlPath::parse("/x/index.html").unwrap();
        assert_eq!(
            route_for_output(&OutputPath::from_route(&exact)).as_str(),
            "/x/"
        );
    }
}
