//! Extraction from eager content trees and realized document elements.

use typst::foundations::Content;
#[cfg(feature = "scan")]
use typst::foundations::Value;
#[cfg(feature = "scan")]
use typst::introspection::MetadataElem;
use typst::loading::DataSource;
use typst::model::{Destination, HeadingElem, LinkElem, LinkTarget};
use typst::visualize::ImageElem;
use typst_html::{HtmlAttr, HtmlElem};

/// Collects values from Typst content in traversal order.
///
/// Every extractor receives the full traversal, including in tuples.
/// First-match collectors ignore later elements without stopping siblings.
pub trait Extractor: Sized {
    /// The type returned after extraction.
    type Output;

    /// Visit one Typst content element.
    fn visit(&mut self, elem: &Content);

    /// Finalize and return the extracted data.
    fn finish(self) -> Self::Output;
}

/// Extract values from an eager Typst content tree.
///
/// Metadata values are payloads, not displayed child content. The metadata
/// element itself is visited; content stored inside its value is not.
#[cfg(feature = "scan")]
pub fn extract<E: Extractor>(content: &Content, mut extractor: E) -> E::Output {
    visit_content(content, &mut |element| extractor.visit(element));
    extractor.finish()
}

#[cfg(feature = "scan")]
pub(super) fn visit_content(content: &Content, visit: &mut impl FnMut(&Content)) {
    visit(content);
    if content.to_packed::<MetadataElem>().is_some() {
        return;
    }
    for (_, value) in content.fields() {
        visit_content_value(value, visit);
    }
}

#[cfg(feature = "scan")]
fn visit_content_value(value: Value, visit: &mut impl FnMut(&Content)) {
    match value {
        Value::Content(content) => visit_content(&content, visit),
        Value::Array(values) => {
            for value in values {
                visit_content_value(value, visit);
            }
        }
        _ => {}
    }
}

/// Extract data from an already realized sequence of document elements.
pub(crate) fn extract_elements<'a, E, I>(elements: I, mut extractor: E) -> E::Output
where
    E: Extractor,
    I: IntoIterator<Item = &'a Content>,
{
    for element in elements {
        extractor.visit(element);
    }
    extractor.finish()
}

macro_rules! impl_extractor_for_tuple {
    ($first:ident $(, $rest:ident)*) => {
        impl<$first: Extractor $(, $rest: Extractor)*> Extractor for ($first, $($rest,)*) {
            type Output = ($first::Output, $($rest::Output,)*);

            #[allow(non_snake_case)]
            fn visit(&mut self, elem: &Content) {
                let ($first, $($rest,)*) = self;
                $first.visit(elem);
                $($rest.visit(elem);)*
            }

            #[allow(non_snake_case)]
            fn finish(self) -> Self::Output {
                let ($first, $($rest,)*) = self;
                ($first.finish(), $($rest.finish(),)*)
            }
        }

        impl_extractor_for_tuple!($($rest),*);
    };
    () => {};
}

impl_extractor_for_tuple!(A, B, C, D, E, F, G, H);

/// Extracts all links from realized content.
#[derive(Debug)]
pub struct LinkExtractor {
    links: Vec<Link>,
    href_attr: HtmlAttr,
    src_attr: HtmlAttr,
}

impl LinkExtractor {
    /// Create a link extractor.
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            href_attr: HtmlAttr::intern("href").expect("href is a valid HTML attribute"),
            src_attr: HtmlAttr::intern("src").expect("src is a valid HTML attribute"),
        }
    }
}

impl Default for LinkExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor for LinkExtractor {
    type Output = Vec<Link>;

    fn visit(&mut self, elem: &Content) {
        if let Some(link) = elem.to_packed::<LinkElem>()
            && let LinkTarget::Dest(Destination::Url(url)) = &link.dest
        {
            self.links.push(Link {
                dest: url.as_str().to_string(),
                source: LinkSource::Link,
            });
        }

        if let Some(html_elem) = elem.to_packed::<HtmlElem>() {
            let attrs = html_elem.attrs.get_cloned(Default::default());
            if let Some(value) = attrs.get(self.href_attr) {
                self.links.push(Link {
                    dest: value.to_string(),
                    source: LinkSource::Href,
                });
            }
            if let Some(value) = attrs.get(self.src_attr) {
                self.links.push(Link {
                    dest: value.to_string(),
                    source: LinkSource::Src,
                });
            }
        }

        if let Some(image) = elem.to_packed::<ImageElem>()
            && let DataSource::Path(path) = &image.source.source
        {
            self.links.push(Link {
                dest: match path {
                    typst::foundations::PathOrStr::Path(path) => {
                        path.vpath().get_with_slash().to_string()
                    }
                    typst::foundations::PathOrStr::Str(path) => path.to_string(),
                },
                source: LinkSource::Image,
            });
        }
    }

    fn finish(self) -> Self::Output {
        self.links
    }
}

/// A link extracted from a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The link destination.
    pub dest: String,
    /// Where this link came from.
    pub source: LinkSource,
}

/// The source of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkSource {
    /// From `#link()`.
    Link,
    /// From an HTML `href` attribute.
    Href,
    /// From an HTML `src` attribute.
    Src,
    /// From `#image()` source data.
    Image,
}

impl Link {
    /// Check if this is an HTTP or HTTPS link.
    pub fn is_http(&self) -> bool {
        self.dest.split_once("://").is_some_and(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
        })
    }

    /// Check if this is an external link.
    pub fn is_external(&self) -> bool {
        self.dest.starts_with("//") || has_uri_scheme(&self.dest)
    }

    /// Check if this is a root-relative link.
    pub fn is_root_relative(&self) -> bool {
        self.dest.starts_with('/') && !self.dest.starts_with("//")
    }

    /// Check if this is a fragment-only link.
    pub fn is_fragment(&self) -> bool {
        self.dest.starts_with('#')
    }

    /// Check if this is a relative link.
    pub fn is_relative(&self) -> bool {
        !self.is_external() && !self.is_root_relative() && !self.is_fragment()
    }
}

fn has_uri_scheme(destination: &str) -> bool {
    let Some((scheme, _)) = destination.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

/// Extracts semantic heading data from realized content.
#[derive(Debug, Default)]
pub struct HeadingExtractor {
    headings: Vec<Heading>,
}

impl HeadingExtractor {
    /// Create a heading extractor.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Extractor for HeadingExtractor {
    type Output = Vec<Heading>;

    fn visit(&mut self, elem: &Content) {
        if let Some(heading) = elem.to_packed::<HeadingElem>() {
            let supplement = heading
                .supplement
                .get_cloned(Default::default())
                .custom()
                .flatten()
                .and_then(|supplement| match supplement {
                    typst::model::Supplement::Content(content) => {
                        Some(content.plain_text().to_string())
                    }
                    typst::model::Supplement::Func(_) => None,
                })
                .filter(|supplement| !supplement.is_empty());
            self.headings.push(Heading {
                level: heading.resolve_level(Default::default()).get() as u8,
                text: heading.body.plain_text().to_string(),
                supplement,
                fragment: None,
                location: heading.location(),
            });
        }
    }

    fn finish(self) -> Self::Output {
        self.headings
    }
}

/// Semantic heading data independent of an export target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Heading level.
    pub level: u8,
    /// Plain heading body text.
    pub text: String,
    /// Resolved plain-text heading supplement, when non-empty.
    pub supplement: Option<String>,
    /// Final export fragment when the owning document provides one.
    pub fragment: Option<String>,
    pub(crate) location: Option<typst::introspection::Location>,
}

#[cfg(test)]
mod tests {
    use super::{Link, LinkExtractor, LinkSource, extract_elements};
    use typst::foundations::NativeElement;
    use typst_html::{HtmlAttr, HtmlAttrs, HtmlElem, HtmlTag};

    #[test]
    fn html_url_attributes_become_links() {
        let mut link_attrs = HtmlAttrs::new();
        link_attrs.push(HtmlAttr::intern("href").unwrap(), "/document/");
        let mut image_attrs = HtmlAttrs::new();
        image_attrs.push(HtmlAttr::intern("src").unwrap(), "/image.png");
        let elements = [
            HtmlElem::new(HtmlTag::intern("a").unwrap())
                .with_attrs(link_attrs)
                .pack(),
            HtmlElem::new(HtmlTag::intern("img").unwrap())
                .with_attrs(image_attrs)
                .pack(),
        ];

        assert_eq!(
            extract_elements(&elements, LinkExtractor::new()),
            vec![
                Link {
                    dest: "/document/".into(),
                    source: LinkSource::Href,
                },
                Link {
                    dest: "/image.png".into(),
                    source: LinkSource::Src,
                },
            ]
        );
    }

    fn link(destination: &str) -> Link {
        Link {
            dest: destination.into(),
            source: LinkSource::Link,
        }
    }

    #[test]
    fn link_destinations_classify_by_scheme() {
        assert!(link("//cdn.example.test/assets/site.css").is_external());
        assert!(!link("//cdn.example.test/assets/site.css").is_root_relative());
        assert!(!link("//cdn.example.test/assets/site.css").is_relative());

        assert!(link("/docs/").is_root_relative());
        assert!(!link("/docs/").is_relative());
        assert!(link("../docs/").is_relative());
        assert!(link("#section").is_fragment());

        assert!(link("HTTPS://example.test").is_http());
        assert!(link("MAILTO:user@example.test").is_external());
        assert!(link("data:text/plain,hello").is_external());
        assert!(link("custom+scheme:value").is_external());
    }
}
