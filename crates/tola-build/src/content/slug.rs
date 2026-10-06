//! Route segments derived from content file layout.
//!
//! File-layout segments retain their hierarchy, and the resulting decoded
//! site-root path passes the shared portable address validation.

use std::borrow::Cow;
use std::path::Path;

use super::{ContentId, ContentSourceLayout};

/// The path a source's route is derived from, with its layout applied and the
/// extension removed.
///
/// This is the only place that knows the layout rule: a directory index is named
/// after its directory, and a single file after its own path without its
/// extension. Components normalize away empty and `.` parts, so no segment can be
/// empty here.
fn route_path(id: &ContentId, layout: ContentSourceLayout) -> Cow<'_, Path> {
    match layout {
        ContentSourceLayout::DirectoryIndex => {
            Cow::Borrowed(id.as_path().parent().unwrap_or_else(|| Path::new("")))
        }
        ContentSourceLayout::SingleFile => Cow::Owned(id.as_path().with_extension("")),
    }
}

/// The original identity segments of a source's route, in order.
///
/// A site program that wants a different route structure starts here rather than
/// re-deriving the layout, which depends on the accepted extension and the index
/// filename.
pub(crate) fn route_segments(id: &ContentId, layout: ContentSourceLayout) -> Vec<String> {
    route_path(id, layout)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_address::{RouteSegmentError, UrlPath, slugify_segments};
    use tola_slugify::NamingRules;

    /// The route the conventional rules recommend, which is what a site gets with
    /// no routing code of its own.
    fn recommendation(path: &str) -> Result<UrlPath, RouteSegmentError> {
        let id = ContentId::new(path.into());
        slugify_segments(
            &route_segments(&id, ContentSourceLayout::from_entry_path(id.as_path())),
            NamingRules::default(),
        )
    }

    #[test]
    fn route_segments_follow_the_source_layout() {
        let cases: [(&str, Vec<&str>); 6] = [
            ("index.typ", vec![]),
            ("about.typ", vec!["about"]),
            ("about/index.typ", vec!["about"]),
            ("posts/deep.typ", vec!["posts", "deep"]),
            ("Café Notes.typ", vec!["Café Notes"]),
            ("posts/Index.typ", vec!["posts", "Index"]),
        ];
        for (path, expected) in cases {
            let id = ContentId::new(path.into());
            let layout = ContentSourceLayout::from_entry_path(id.as_path());
            assert_eq!(
                route_segments(&id, layout),
                expected
                    .iter()
                    .map(|part| (*part).to_owned())
                    .collect::<Vec<_>>(),
                "{path}"
            );
        }
    }

    /// Routes for source names that need slugifying.
    ///
    /// Every segment is slugified on its own, so a directory index names its directory and a
    /// single file keeps the hierarchy and adds its extension-stripped name.
    #[test]
    fn routes_slugify_every_segment() {
        for (source, expected) in [
            ("index.typ", "/"),
            ("Café Notes.typ", "/café-notes/"),
            ("hello world/deep.typ", "/hello-world/deep/"),
            ("北京/notes.typ", "/北京/notes/"),
            ("posts/index.typ", "/posts/"),
            (
                "drafts/release-notes.post.typ",
                "/drafts/release-notes.post/",
            ),
        ] {
            assert_eq!(
                recommendation(source).unwrap().as_str(),
                expected,
                "{source}"
            );
        }
    }
}
