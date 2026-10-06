//! The decoded site-root route ordered identity segments name.
//!
//! Each segment is transformed on its own by [`tola_slugify`], and the joined page path is
//! validated here, where paths belong.

use tola_slugify::{NamingRules, slugify_segment};

use super::{UrlPath, UrlPathError};

/// Why ordered identity segments cannot name a decoded site-root route.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouteSegmentError {
    /// One segment holds nothing a name can be made from.
    #[error("segment {index} (`{text}`) has nothing that can name a path segment")]
    Empty { index: usize, text: String },
    /// The slugged segments are not a valid decoded site-root page path.
    #[error("{0}")]
    Path(#[from] UrlPathError),
}

/// Slug each identity segment in order and validate the joined decoded page path.
///
/// Every segment is transformed on its own, so no segment can change the hierarchy
/// or introduce a second one. Zero segments name the site root. A nonempty segment
/// that becomes empty is an error, not a missing directory or an alternate spelling
/// of the root.
pub fn slugify_segments(
    segments: &[String],
    rules: NamingRules,
) -> Result<UrlPath, RouteSegmentError> {
    let mut path = String::from("/");
    for (position, text) in segments.iter().enumerate() {
        let name = slugify_segment(text, rules).ok_or_else(|| RouteSegmentError::Empty {
            index: position + 1,
            text: text.clone(),
        })?;
        path.push_str(&name);
        path.push('/');
    }
    Ok(UrlPath::from_decoded(&path)?)
}

#[cfg(test)]
mod tests {
    use tola_slugify::{SlugCase, SlugMode, SlugSeparator};

    use super::*;

    /// The rules a naming case names, with everything else conventional.
    fn rules(mode: SlugMode, case: SlugCase, separator: SlugSeparator) -> NamingRules {
        NamingRules {
            mode,
            case,
            separator,
            ..Default::default()
        }
    }

    fn slugged_route(
        segments: &[&str],
        mode: SlugMode,
        case: SlugCase,
        separator: SlugSeparator,
    ) -> Result<UrlPath, RouteSegmentError> {
        let segments = segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect::<Vec<_>>();
        slugify_segments(&segments, rules(mode, case, separator))
    }

    #[test]
    fn no_segments_name_the_site_root() {
        assert_eq!(
            slugged_route(&[], SlugMode::Unicode, SlugCase::Lower, SlugSeparator::Dash)
                .unwrap()
                .as_str(),
            "/"
        );
    }

    #[test]
    fn empty_segments_report_their_index() {
        for (segments, index) in [
            (vec!["#"], 1usize),
            (vec!["posts", "#"], 2),
            (vec!["posts", "#", "child"], 2),
        ] {
            assert_eq!(
                slugged_route(
                    &segments,
                    SlugMode::Unicode,
                    SlugCase::Lower,
                    SlugSeparator::Dash
                )
                .unwrap_err(),
                RouteSegmentError::Empty {
                    index,
                    text: "#".to_owned(),
                },
                "{segments:?}"
            );
        }
    }

    #[test]
    fn slugged_dot_segments_are_refused() {
        for segments in [vec!["posts", "#.#"], vec!["posts", "#..#", "child"]] {
            assert_eq!(
                slugged_route(
                    &segments,
                    SlugMode::Unicode,
                    SlugCase::Lower,
                    SlugSeparator::Dash
                )
                .unwrap_err(),
                RouteSegmentError::Path(UrlPathError::DotSegment),
                "{segments:?}"
            );
        }
    }

    #[test]
    fn route_preserves_unicode_hierarchy() {
        assert_eq!(
            slugged_route(
                &["北京", "ΟΣ Café"],
                SlugMode::Unicode,
                SlugCase::Lower,
                SlugSeparator::Dash
            )
            .unwrap()
            .as_str(),
            "/北京/ος-café/"
        );
    }

    #[test]
    fn nonportable_segments_report_error() {
        for (segments, expected) in [
            (
                vec!["posts", "file.."],
                UrlPathError::FilesystemTrailingDotOrSpace,
            ),
            (vec!["posts", "CON"], UrlPathError::WindowsDeviceName),
        ] {
            assert_eq!(
                slugged_route(
                    &segments,
                    SlugMode::Unicode,
                    SlugCase::Lower,
                    SlugSeparator::Dash
                )
                .unwrap_err(),
                RouteSegmentError::Path(expected),
                "{segments:?}"
            );
        }
        let long = "a".repeat(256);
        assert!(matches!(
            slugged_route(
                &[long.as_str()],
                SlugMode::Unicode,
                SlugCase::Lower,
                SlugSeparator::Dash
            )
            .unwrap_err(),
            RouteSegmentError::Path(UrlPathError::OutputSegmentTooLong { .. })
        ));
    }

    #[test]
    fn each_segment_keeps_its_own_name() {
        assert_eq!(
            slugged_route(
                &["posts", "北京", "世界"],
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash
            )
            .unwrap()
            .as_str(),
            "/posts/bei-jing/shi-jie/"
        );
    }
}
