//! Classify raw link strings by their syntactic form.

#[inline]
fn is_external_link(link: &str) -> bool {
    link.starts_with("//")
        || link.split_once(':').is_some_and(|(scheme, _)| {
            let mut characters = scheme.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && characters.all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
                })
        })
}

/// Path, query, and fragment of one destination, with escapes preserved.
pub struct DestinationParts<'a> {
    /// Destination path with the query and fragment removed.
    pub path: &'a str,
    /// Query without the leading question mark.
    pub query: Option<&'a str>,
    /// Fragment without the leading hash sign, with escapes preserved.
    pub fragment: Option<&'a str>,
}

#[inline]
pub fn split_destination(destination: &str) -> DestinationParts<'_> {
    let (path_and_query, fragment) = destination
        .split_once('#')
        .map_or((destination, None), |(path, fragment)| {
            (path, Some(fragment))
        });
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
    DestinationParts {
        path,
        query,
        fragment,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind<'a> {
    /// Link with a URL scheme (`https:`, `mailto:`, etc.) or network-path prefix (`//`).
    External(&'a str),
    /// Current-page fragment (`#section` or `./#section`), without the prefix.
    Fragment(&'a str),
    /// Site-root-relative path (`/about`, `/posts/hello`).
    SiteRoot(&'a str),
    /// File-relative path (`./image.png`, `../other`).
    FileRelative(&'a str),
}

/// The resource kind a reference must resolve to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequiredResourceKind {
    /// The destination may be an HTML document or a non-document resource.
    DocumentOrResource,
    /// The destination must be a non-document resource.
    NonDocumentResource,
}

impl<'a> LinkKind<'a> {
    #[inline]
    pub fn parse(link: &'a str) -> Self {
        if is_external_link(link) {
            Self::External(link)
        } else if let Some(anchor) = link.strip_prefix('#') {
            Self::Fragment(anchor)
        } else if let Some(anchor) = link.strip_prefix("./#") {
            Self::Fragment(anchor)
        } else if link.starts_with('/') {
            Self::SiteRoot(link)
        } else {
            Self::FileRelative(link)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_spellings_get_their_kind() {
        for (link, expected) in [
            (
                "https://example.com",
                LinkKind::External("https://example.com"),
            ),
            (
                "mailto:user@example.com",
                LinkKind::External("mailto:user@example.com"),
            ),
            ("tel:+1234567890", LinkKind::External("tel:+1234567890")),
            (
                "//cdn.example.com/image.png",
                LinkKind::External("//cdn.example.com/image.png"),
            ),
            ("#section", LinkKind::Fragment("section")),
            ("#my-heading", LinkKind::Fragment("my-heading")),
            ("#", LinkKind::Fragment("")),
            ("./#section", LinkKind::Fragment("section")),
            ("./#my-heading", LinkKind::Fragment("my-heading")),
            ("./#", LinkKind::Fragment("")),
            ("/about", LinkKind::SiteRoot("/about")),
            ("/posts/hello", LinkKind::SiteRoot("/posts/hello")),
            ("/about#team", LinkKind::SiteRoot("/about#team")),
            ("./image.png", LinkKind::FileRelative("./image.png")),
            ("../other", LinkKind::FileRelative("../other")),
            ("image.png", LinkKind::FileRelative("image.png")),
            ("./page#section", LinkKind::FileRelative("./page#section")),
            ("?page=2", LinkKind::FileRelative("?page=2")),
        ] {
            assert_eq!(LinkKind::parse(link), expected, "{link}");
        }
    }

    #[test]
    fn browser_destination_splits() {
        let parts = split_destination("/search/?q=rust#res%20ults");

        assert_eq!(parts.path, "/search/");
        assert_eq!(parts.query, Some("q=rust"));
        assert_eq!(parts.fragment, Some("res%20ults"));

        let bare = split_destination("/search/");
        assert_eq!(bare.path, "/search/");
        assert_eq!(bare.query, None);
        assert_eq!(bare.fragment, None);
    }
}
