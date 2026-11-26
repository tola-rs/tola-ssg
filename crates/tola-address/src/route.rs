//! Decoded site-root route from ordered identity segments.

use tola_slugify::{NamingRules, slugify_segment};

use super::{UrlPath, UrlPathError};

/// Error turning ordered identity segments into a decoded site-root route.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouteSegmentError {
    /// Segment produced no slug characters.
    #[error("segment {index} (`{text}`) has nothing that can name a path segment")]
    Empty { index: usize, text: String },
    /// Slugged segments failed decoded URL path validation.
    #[error("{0}")]
    Path(#[from] UrlPathError),
}

/// Slug each identity segment independently and validate the joined decoded page path.
pub fn slugify_segments(
    segments: &[String],
    rules: NamingRules,
) -> Result<UrlPath, RouteSegmentError> {
    let mut path = String::from("/");
    for (idx, text) in segments.iter().enumerate() {
        let name = slugify_segment(text, rules).ok_or_else(|| RouteSegmentError::Empty {
            index: idx + 1,
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

    fn slugged_route(segments: &[&str], mode: SlugMode) -> Result<UrlPath, RouteSegmentError> {
        let owned: Vec<String> = segments.iter().map(|s| (*s).to_owned()).collect();
        let rules = NamingRules {
            mode,
            case: SlugCase::Lower,
            separator: SlugSeparator::Dash,
            ..Default::default()
        };
        slugify_segments(&owned, rules)
    }

    #[test]
    fn empty_segments_root() {
        assert_eq!(slugged_route(&[], SlugMode::Unicode).unwrap().as_str(), "/");
    }

    #[test]
    fn dot_segments_error() {
        for segments in [vec!["posts", "#.#"], vec!["posts", "#..#", "child"]] {
            assert_eq!(
                slugged_route(&segments, SlugMode::Unicode).unwrap_err(),
                RouteSegmentError::Path(UrlPathError::DotSegment),
                "{segments:?}"
            );
        }
    }

    #[test]
    fn unicode_hierarchy() {
        assert_eq!(
            slugged_route(&["北京", "ΟΣ Café"], SlugMode::Unicode)
                .unwrap()
                .as_str(),
            "/北京/ος-café/"
        );
    }

    #[test]
    fn ascii_segments() {
        assert_eq!(
            slugged_route(&["posts", "北京", "世界"], SlugMode::Ascii)
                .unwrap()
                .as_str(),
            "/posts/bei-jing/shi-jie/"
        );
    }
}
