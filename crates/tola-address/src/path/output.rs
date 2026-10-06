//! Portable logical identities below one site output root.

use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use thiserror::Error;

use super::portable::validate_portable_segment;
use super::{PortablePathError, UrlPath, portable_collision_key};

/// The one segment Tola reserves at the top of every published output root.
///
/// `OutputPath` rejects a non-system output that begins with this segment, and
/// system assets publish beneath it. The build engine names the same directory
/// for producers.
pub const RESERVED_ROOT: &str = "_tola";

/// A portable, non-empty file path relative to the site output root.
///
/// The path is decoded text: a literal `%` or `#` is a filename character, not URL syntax, and
/// only [`UrlPath`] renders it for a browser. Its owner determines whether it denotes a file or a
/// directory tree; a trailing slash never appears in the value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputPath(Arc<str>);

impl Serialize for OutputPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OutputPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(|error| de::Error::custom(error.to_string()))
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
    /// Parse one decoded relative file path.
    ///
    /// The text is a path, never a URL: no percent decoding and no query or fragment interpretation
    /// happens, so a portable literal `%` or `#` names a file. Structural and portable rules stay:
    /// the path is relative, non-empty, has no trailing slash, and every segment is a portable
    /// filename.
    pub fn parse(decoded_relative: &str) -> Result<Self, OutputPathError> {
        if decoded_relative.contains('\\') {
            return Err(OutputPathError::Backslash);
        }
        if decoded_relative.chars().any(is_control) {
            return Err(OutputPathError::Control);
        }
        if decoded_relative.starts_with('/') {
            return Err(OutputPathError::Absolute);
        }
        if decoded_relative.is_empty() || decoded_relative.ends_with('/') {
            return Err(OutputPathError::Empty);
        }

        for (index, segment) in decoded_relative.split('/').enumerate() {
            validate_segment(segment, index == 0)?;
        }

        Ok(Self(Arc::from(decoded_relative)))
    }

    /// The logical output file one decoded site-root route names.
    ///
    /// A trailing slash names a directory published as its `index.html`; a route without one names
    /// a file exactly as spelled. `/` is the root directory, so it publishes `index.html`. The
    /// route is already validated, so this cannot fail.
    pub fn from_route(route: &UrlPath) -> Self {
        let path = route.as_str();
        let relative = if path == "/" {
            "index.html".to_owned()
        } else if let Some(directory) = path.strip_suffix('/') {
            format!("{}/index.html", directory.trim_start_matches('/'))
        } else {
            path.trim_start_matches('/').to_owned()
        };
        Self(Arc::from(relative))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Append a validated descendant without reparsing either path.
    pub fn join(&self, descendant: &Self) -> Self {
        Self(Arc::from(format!(
            "{}/{}",
            self.as_str(),
            descendant.as_str()
        )))
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

/// The decoded site-root route one logical output file is reachable at.
///
/// The inverse of [`OutputPath::from_route`] for every output: an exact `index.html` filename
/// becomes the directory address that contains it (`index.html` is `/`, `x/index.html` is
/// `/x/`), and every other file keeps its path under `/`. The output is already validated, so
/// this cannot fail.
pub fn route_for_output(output: &OutputPath) -> UrlPath {
    let path = output.as_str();
    let route = if path == "index.html" {
        "/".to_owned()
    } else if let Some(directory) = path.strip_suffix("/index.html") {
        format!("/{directory}/")
    } else {
        format!("/{path}")
    };
    UrlPath::from_validated_decoded(route)
}

/// The exact file URL one logical output is served at.
///
/// An `index.html` output is also reachable through the directory route [`route_for_output`]
/// yields, so this exact address is what distinguishes the file from that route. The output is
/// already validated, so this cannot fail.
pub fn asset_url_from_output(output: &OutputPath) -> UrlPath {
    UrlPath::from_validated_decoded(format!("/{}", output.as_str()))
}

/// The output one site-root file URL names, undoing [`asset_url_from_output`].
///
/// A directory-shaped URL names no file, so `/` and `/x/` are refused.
pub fn asset_output_from_url(url: &UrlPath) -> Result<OutputPath, OutputPathError> {
    OutputPath::parse(url.as_str().trim_start_matches('/'))
}

/// The directory route a published `index.html` file is also reachable through.
///
/// The exact file URL is the address an output registers, so an alias lookup keys this extra
/// route, not the exact URL again. Only an `index.html`-shaped output has a second address:
/// `x/index.html` yields `/x/` and `index.html` yields `/`, while every other output yields
/// `None`.
pub fn asset_directory_index_alias(output: &OutputPath) -> Option<UrlPath> {
    let exact = asset_url_from_output(output);
    let route = route_for_output(output);
    (route != exact).then_some(route)
}

/// Whether a portable collision key names a path in the namespace Tola reserves.
pub fn portable_key_is_reserved(portable_key: &[String]) -> bool {
    portable_key
        .first()
        .is_some_and(|segment| segment == RESERVED_ROOT)
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

fn validate_segment(segment: &str, first: bool) -> Result<(), OutputPathError> {
    if segment.is_empty() {
        return Err(OutputPathError::EmptyComponent);
    }
    if matches!(segment, "." | "..") {
        return Err(OutputPathError::DotComponent);
    }
    validate_portable_segment(segment, first)?;
    Ok(())
}

fn is_control(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_paths_are_refused() {
        assert_eq!(
            OutputPath::parse("/tags/rust/index.html"),
            Err(OutputPathError::Absolute)
        );
    }

    #[test]
    fn ambiguous_output_paths_are_refused() {
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
    fn portable_literals_stay_filenames() {
        for path in ["100%.html", "a#b.html", "notes%20here"] {
            assert_eq!(OutputPath::parse(path).unwrap().as_str(), path);
        }
    }

    #[test]
    fn routes_name_directory_indexes() {
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
    fn index_files_become_directory_routes() {
        for (output, route) in [
            ("index.html", "/"),
            ("posts/rust/index.html", "/posts/rust/"),
            ("404.html", "/404.html"),
            ("notes", "/notes"),
            ("paper.pdf", "/paper.pdf"),
        ] {
            let output = OutputPath::parse(output).unwrap();
            assert_eq!(route_for_output(&output).as_str(), route);
        }
    }

    #[test]
    fn route_and_output_round_trip() {
        // `parse` reads the browser spelling, where the filename's literal `%` is escaped.
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

    #[test]
    fn unicode_names_stay_decoded() {
        let route = UrlPath::parse("/%E4%B8%AD%E6%96%87/").unwrap();
        let output = OutputPath::from_route(&route);

        assert_eq!(output.as_str(), "中文/index.html");
        assert_eq!(route_for_output(&output).as_str(), "/中文/");
        assert_eq!(route.to_encoded(), "/%E4%B8%AD%E6%96%87/");
    }

    #[test]
    fn asset_urls_spell_the_exact_file() {
        for (output, url) in [
            ("x/index.html", "/x/index.html"),
            ("index.html", "/index.html"),
            ("images/logo.png", "/images/logo.png"),
        ] {
            assert_eq!(
                asset_url_from_output(&OutputPath::parse(output).unwrap()),
                url
            );
        }
    }

    #[test]
    fn file_urls_round_trip_to_their_output() {
        for output in ["x/index.html", "index.html", "images/logo.png", "100%.html"] {
            let output = OutputPath::parse(output).unwrap();
            assert_eq!(
                asset_output_from_url(&asset_url_from_output(&output)).unwrap(),
                output
            );
        }
        for url in ["/", "/x/"] {
            let url = UrlPath::parse(url).unwrap();
            assert!(asset_output_from_url(&url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn directory_index_aliases_name_the_route() {
        for (output, alias) in [
            ("x/index.html", Some("/x/")),
            ("index.html", Some("/")),
            ("posts/rust/index.html", Some("/posts/rust/")),
            ("x", None),
            ("x.pdf", None),
            ("404.html", None),
        ] {
            let output = OutputPath::parse(output).unwrap();
            let alias = alias.map(|alias| UrlPath::parse(alias).unwrap());
            assert_eq!(asset_directory_index_alias(&output), alias, "{output}");
        }
    }
}
