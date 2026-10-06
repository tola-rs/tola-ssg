//! Selection and Typst serialization of compiled HTML subtrees.

use std::collections::BTreeMap;

use comemo::Track;
use typst::layout::{Frame, FrameItem};
use typst::model::{Destination, LateLinkResolver};
use typst::syntax::VirtualPath;
use typst_html::{HtmlDocument, HtmlElement, HtmlNode, HtmlOptions};

use super::reference::{
    HtmlReferenceUse, attribute, classify_reference_use, map_url_ranges, unique_attributes,
    url_ranges,
};
use crate::{BundleCancellation, CompiledBundleDocument};

/// HTML encoding options and ancestor retention for a selection.
#[derive(Clone, Debug, Default)]
pub struct HtmlFragmentOptions {
    /// Options passed to Typst's HTML serializer.
    pub html: HtmlOptions,
    /// Keep ancestor containers and their attributes, with document-level
    /// containers represented by divs.
    pub preserve_ancestors: bool,
}

/// A compiled selection and its original document's style dependencies.
///
/// Styles retain inline CSS text and external stylesheet dependencies; attaching
/// them is the consumer's choice. A copied subtree does not reproduce its full
/// page's CSS cascade, and URLs inside CSS are not rewritten, so those styles
/// need the original document base.
#[derive(Clone, Debug)]
pub struct HtmlFragmentExport {
    /// Original logical Bundle output, whose public address belongs to the host.
    pub document: VirtualPath,
    /// Raw first active base href, resolved relative to the original document address.
    pub base_href: Option<String>,
    /// The selection, with ancestry retained if requested.
    pub html: String,
    /// Encoded head styles and stylesheet links, in document order.
    pub styles: Vec<String>,
    /// Original outer-to-inner ancestors without children, including their native
    /// tags and attributes.
    pub ancestors: Vec<HtmlElement>,
}

/// A subtree of one compiled HTML document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HtmlFragmentSelection {
    /// The document body, represented by a neutral `div` with its attributes.
    Body,
    /// One complete HTML element with this exact, decoded DOM ID.
    Id(String),
}

/// A compiled document could not supply the requested HTML fragment.
#[derive(Debug, thiserror::Error)]
pub enum HtmlFragmentError {
    /// The caller cancelled selection or export.
    #[error("HTML fragment export was cancelled")]
    Cancelled,
    /// The selected Bundle document uses a paged export kind.
    #[error("Bundle document `{}` is not HTML", path.get_with_slash())]
    NotHtmlDocument {
        /// Logical Bundle output path.
        path: VirtualPath,
    },
    /// No body is present in the compiled HTML DOM.
    #[error("the document has no `body` element")]
    MissingBody,
    /// More than one body is present in the compiled HTML DOM.
    #[error("the document has more than one `body` element")]
    RepeatedBody,
    /// No HTML element has the requested ID. IDs inside SVG frames are not subtree roots.
    #[error("the document has no element with id `{id}`")]
    MissingId {
        /// The requested decoded DOM ID.
        id: String,
    },
    /// The ID does not select a unique HTML element.
    #[error("the document has more than one element with id `{id}`")]
    RepeatedId {
        /// The ambiguous decoded DOM ID.
        id: String,
    },
    /// The host could not rewrite one URL-bearing attribute or frame link.
    #[error("HTML fragment reference: {message}")]
    InvalidReference {
        /// Host-provided explanation.
        message: String,
    },
    /// Typst's HTML serializer rejected the selected DOM.
    #[error("could not export the HTML fragment")]
    Export {
        /// Typst diagnostics, including their source spans and hints.
        diagnostics: Vec<crate::NativeDiagnostic>,
    },
}

impl crate::diagnostic::CancelledError for HtmlFragmentError {
    fn from_cancellation() -> Self {
        Self::Cancelled
    }
}

impl HtmlFragmentError {
    /// Whether export stopped because the caller cancelled it.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Typst diagnostics when the serializer rejected the DOM.
    pub fn raw_diagnostics(&self) -> Option<&[crate::NativeDiagnostic]> {
        match self {
            Self::Export { diagnostics } => Some(diagnostics),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum ElementSelection {
    Unique(Vec<usize>),
    Repeated,
}

/// Structural paths into one immutable native document; no DOM nodes are copied.
#[derive(Debug, Default)]
pub(crate) struct HtmlFragmentIndex {
    body: Option<ElementSelection>,
    ids: BTreeMap<String, ElementSelection>,
    styles: Vec<Vec<usize>>,
    base_href: Option<String>,
}

impl HtmlFragmentIndex {
    pub(crate) fn new(
        document: &HtmlDocument,
        cancellation: &BundleCancellation,
    ) -> Result<Self, HtmlFragmentError> {
        let mut index = Self::default();
        index.visit(document.root(), &mut Vec::new(), false, cancellation)?;
        Ok(index)
    }

    fn visit(
        &mut self,
        element: &HtmlElement,
        path: &mut Vec<usize>,
        in_head: bool,
        cancellation: &BundleCancellation,
    ) -> Result<(), HtmlFragmentError> {
        cancellation.ensure_active_as()?;
        let tag = element.tag.resolve();
        if self.base_href.is_none() && tag.as_str().eq_ignore_ascii_case("base") {
            self.base_href = attribute(element, "href").map(str::to_owned);
        }
        if in_head
            && (tag.as_str().eq_ignore_ascii_case("style")
                || tag.as_str().eq_ignore_ascii_case("link")
                    && attribute(element, "rel").is_some_and(|rel| {
                        rel.split_ascii_whitespace()
                            .any(|part| part.eq_ignore_ascii_case("stylesheet"))
                    }))
        {
            self.styles.push(path.clone());
        }
        if element.tag.resolve().as_str().eq_ignore_ascii_case("body") {
            self.body = Some(match self.body.take() {
                None => ElementSelection::Unique(path.clone()),
                Some(_) => ElementSelection::Repeated,
            });
        }
        if let Some(id) = attribute(element, "id").filter(|id| !id.is_empty()) {
            self.ids
                .entry(id.to_owned())
                .and_modify(|selection| *selection = ElementSelection::Repeated)
                .or_insert_with(|| ElementSelection::Unique(path.clone()));
        }
        if element
            .tag
            .resolve()
            .as_str()
            .eq_ignore_ascii_case("template")
        {
            return Ok(());
        }
        for (position, child) in element.children.iter().enumerate() {
            cancellation.ensure_active_as()?;
            if let HtmlNode::Element(child) = child {
                path.push(position);
                self.visit(
                    child,
                    path,
                    in_head || tag.as_str().eq_ignore_ascii_case("head"),
                    cancellation,
                )?;
                path.pop();
            }
        }
        Ok(())
    }

    fn selection_path(
        &self,
        selection: &HtmlFragmentSelection,
    ) -> Result<&[usize], HtmlFragmentError> {
        Ok(match selection {
            HtmlFragmentSelection::Body => match &self.body {
                Some(ElementSelection::Unique(path)) => path,
                Some(ElementSelection::Repeated) => return Err(HtmlFragmentError::RepeatedBody),
                None => return Err(HtmlFragmentError::MissingBody),
            },
            HtmlFragmentSelection::Id(id) => match self.ids.get(id) {
                Some(ElementSelection::Unique(path)) => path,
                Some(ElementSelection::Repeated) => {
                    return Err(HtmlFragmentError::RepeatedId { id: id.clone() });
                }
                None => return Err(HtmlFragmentError::MissingId { id: id.clone() }),
            },
        })
    }

    fn element<'a>(&self, document: &'a HtmlDocument, path: &[usize]) -> &'a HtmlElement {
        let mut element = document.root();
        for &position in path {
            let Some(HtmlNode::Element(child)) = element.children.get(position) else {
                unreachable!("fragment index belongs to this immutable document")
            };
            element = child;
        }
        element
    }
}

pub(crate) fn export_fragment(
    document: &HtmlDocument,
    owner: &CompiledBundleDocument<'_>,
    index: &HtmlFragmentIndex,
    selection: &HtmlFragmentSelection,
    options: &HtmlFragmentOptions,
    cancellation: &BundleCancellation,
    rewrite: &mut dyn FnMut(&str, HtmlReferenceUse) -> Result<String, HtmlFragmentError>,
) -> Result<HtmlFragmentExport, HtmlFragmentError> {
    cancellation.ensure_active_as()?;
    let selected_path = index.selection_path(selection)?;
    let mut root = index.element(document, selected_path).clone();
    if matches!(selection, HtmlFragmentSelection::Body) || options.preserve_ancestors {
        normalize_document_container(&mut root);
    }
    let resolver = owner.link_resolver();
    rewrite_element(&mut root, &resolver, cancellation, rewrite)?;
    let mut ancestors = Vec::new();
    let mut parent = document.root();
    for &position in selected_path {
        cancellation.ensure_active_as()?;
        let mut ancestor = parent.clone();
        ancestor.children = Default::default();
        ancestors.push(ancestor);
        let HtmlNode::Element(child) = &parent.children[position] else {
            unreachable!("fragment ancestry was indexed from native elements")
        };
        parent = child;
    }
    if options.preserve_ancestors {
        for ancestor in ancestors.iter().rev() {
            let mut wrapper = ancestor.clone();
            normalize_document_container(&mut wrapper);
            rewrite_element(&mut wrapper, &resolver, cancellation, rewrite)?;
            wrapper.children.push(root.into());
            root = wrapper;
        }
    }
    let html = encode_fragment(&root, &options.html, cancellation, &resolver)?;
    let mut styles = Vec::with_capacity(index.styles.len());
    for style_path in &index.styles {
        cancellation.ensure_active_as()?;
        if style_path.starts_with(selected_path) {
            continue;
        }
        let mut style = index.element(document, style_path).clone();
        rewrite_element(&mut style, &resolver, cancellation, rewrite)?;
        styles.push(encode_fragment(
            &style,
            &options.html,
            cancellation,
            &resolver,
        )?);
    }
    Ok(HtmlFragmentExport {
        document: owner.path().clone(),
        base_href: index.base_href.clone(),
        html,
        styles,
        ancestors,
    })
}

fn encode_fragment(
    root: &HtmlElement,
    options: &HtmlOptions,
    cancellation: &BundleCancellation,
    resolver: &LateLinkResolver<'_>,
) -> Result<String, HtmlFragmentError> {
    cancellation.ensure_active_as()?;
    let encoded =
        typst_html::html_in_bundle(root, options, resolver.track()).map_err(|diagnostics| {
            HtmlFragmentError::Export {
                diagnostics: diagnostics.into_iter().map(Into::into).collect(),
            }
        })?;
    cancellation.ensure_active_as()?;
    // Typst prefixes a document declaration, which a fragment does not need.
    Ok(encoded
        .strip_prefix("<!DOCTYPE html>")
        .expect("official HTML encoding starts with its document declaration")
        .to_owned())
}

fn normalize_document_container(element: &mut HtmlElement) {
    if ["html", "head", "body"]
        .iter()
        .any(|tag| element.tag.resolve().as_str().eq_ignore_ascii_case(tag))
    {
        element.tag = typst_html::tag::div;
    }
}

fn rewrite_element(
    element: &mut HtmlElement,
    resolver: &LateLinkResolver<'_>,
    cancellation: &BundleCancellation,
    rewrite: &mut dyn FnMut(&str, HtmlReferenceUse) -> Result<String, HtmlFragmentError>,
) -> Result<(), HtmlFragmentError> {
    cancellation.ensure_active_as()?;
    let tag = element.tag.resolve().as_str().to_ascii_lowercase();
    let mut replacements = Vec::new();
    for attribute in unique_attributes(element) {
        cancellation.ensure_active_as()?;
        let Some(reference_use) = classify_reference_use(element, &tag, &attribute.name) else {
            continue;
        };
        let rewritten = map_url_ranges(
            attribute.value,
            url_ranges(&tag, &attribute.name, attribute.value),
            &mut |uri| rewrite(uri, reference_use).map(Some),
        )?;
        if let Some(rewritten) = rewritten {
            replacements.push((attribute.index, rewritten));
        }
    }
    for (index, rewritten) in replacements {
        element.attrs.0.make_mut()[index].1 = rewritten.into();
    }
    if tag == "template" {
        return Ok(());
    }
    for child in element.children.make_mut() {
        cancellation.ensure_active_as()?;
        match child {
            HtmlNode::Element(child) => rewrite_element(child, resolver, cancellation, rewrite)?,
            HtmlNode::Frame(frame) => {
                rewrite_frame(&mut frame.inner, resolver, cancellation, rewrite)?
            }
            HtmlNode::Tag(_) | HtmlNode::Text(..) => {}
        }
    }
    Ok(())
}

fn rewrite_frame(
    frame: &mut Frame,
    resolver: &LateLinkResolver<'_>,
    cancellation: &BundleCancellation,
    rewrite: &mut dyn FnMut(&str, HtmlReferenceUse) -> Result<String, HtmlFragmentError>,
) -> Result<(), HtmlFragmentError> {
    cancellation.ensure_active_as()?;
    let mut failure = None;
    // Native Frame exposes mutable entries through retain; preserve every
    // entry and change only URI destinations in this private cloned subtree.
    frame.retain(|entry| {
        if failure.is_some() {
            return true;
        }
        let rewritten = (|| {
            cancellation.ensure_active_as()?;
            match entry {
                FrameItem::Group(group) => {
                    rewrite_frame(&mut group.frame, resolver, cancellation, rewrite)?
                }
                FrameItem::Link(destination, _) => {
                    let uri = match destination {
                        Destination::Url(uri) => Some(uri.to_string()),
                        Destination::Location(location) => resolver
                            .resolve(*location)
                            .and_then(|resolved| resolved.into_relative_uri().ok())
                            .map(|uri| uri.to_string()),
                        Destination::Position(_) => None,
                    };
                    if let Some(uri) = uri {
                        let uri =
                            typst::model::Url::new(rewrite(&uri, HtmlReferenceUse::Navigation)?)
                                .map_err(|error| HtmlFragmentError::InvalidReference {
                                    message: error.to_string(),
                                })?;
                        *destination = Destination::Url(uri);
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = rewritten {
            failure = Some(error);
        }
        true
    });
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(source: &str) -> (tempfile::TempDir, crate::TypstWorld) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("site.typ");
        std::fs::write(&path, source).unwrap();
        let world = crate::TypstWorld::builder(&path, directory.path())
            .no_fonts()
            .with_local_cache()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        (directory, world)
    }
    #[test]
    fn fragment_styles_exclude_head_scripts() {
        let (_directory, world) = world(
            r#"#document("index.html", html.html(
  html.head(
    html.style(".shell { color: red; }")
    + html.link(rel: "stylesheet", href: "theme.css")
    + html.script("headScript()")
  ) + html.body(id: "body", class: "shell", lang: "zh")[
    #html.article(id: "article")[Selected]
    #html.p[Outside]
  ]
))"#,
        );
        let cancellation = BundleCancellation::default();
        let compilation = crate::compile_bundle_world(&world, &cancellation).unwrap();
        let document = compilation.documents().next().unwrap();
        let options = HtmlFragmentOptions {
            preserve_ancestors: true,
            ..Default::default()
        };
        let fragment = document
            .html_fragment_with_links(
                &HtmlFragmentSelection::Id("article".into()),
                &options,
                &cancellation,
                &mut |uri, usage| {
                    assert_eq!(usage, HtmlReferenceUse::Stylesheet);
                    Ok(format!("https://example.test/{uri}"))
                },
            )
            .unwrap();
        assert!(fragment.html.contains("class=\"shell\""), "{fragment:?}");
        assert!(fragment.html.contains("lang=\"zh\""));
        assert!(!fragment.html.contains("Outside"));
        assert_eq!(fragment.styles.len(), 2);
        assert!(fragment.styles[0].contains(".shell"));
        assert!(fragment.styles[0].contains("color: red"));
        assert!(fragment.styles[1].contains("https://example.test/theme.css"));
        assert!(
            !fragment
                .styles
                .iter()
                .any(|style| style.contains("headScript"))
        );
        assert!(
            fragment
                .ancestors
                .iter()
                .all(|ancestor| ancestor.children.is_empty())
        );
        assert!(
            fragment
                .ancestors
                .iter()
                .any(|ancestor| ancestor.tag == typst_html::tag::body)
        );
        let body = document
            .html_fragment(
                &HtmlFragmentSelection::Id("body".into()),
                &options,
                &cancellation,
            )
            .unwrap();
        assert!(!body.html.contains("<body"));
        assert!(body.html.contains("class=\"shell\""));
    }

    #[test]
    fn srcset_rewrite_keeps_descriptors() {
        let source = "  first.png 1x, data:image/png;base64,AAAA 2x, third.png 3x";
        let rewritten = map_url_ranges(source, url_ranges("img", "srcset", source), &mut |url| {
            Ok::<_, HtmlFragmentError>(Some(if url.starts_with("data:") {
                url.to_owned()
            } else {
                format!("https://example.test/{url}")
            }))
        })
        .unwrap()
        .unwrap_or_else(|| source.to_owned());
        assert_eq!(
            rewritten,
            "  https://example.test/first.png 1x, data:image/png;base64,AAAA 2x, https://example.test/third.png 3x"
        );
    }

    #[test]
    fn fragment_export_keeps_document() {
        let (_directory, world) = world(
            r#"#document("index.html")[
  #html.article(id: "article")[#html.p[Hello #html.strong[world]] #html.a(href: "next/")[Next]]
  #html.p[Outside]
]"#,
        );
        let cancellation = BundleCancellation::default();
        let compilation = crate::compile_bundle_world(&world, &cancellation).unwrap();
        let page = compilation.documents().next().unwrap();
        let selection = HtmlFragmentSelection::Id("article".into());
        let html = page
            .html_fragment_with_links(
                &selection,
                &HtmlFragmentOptions::default(),
                &cancellation,
                &mut |url, _| Ok(format!("https://example.test/{url}")),
            )
            .unwrap();
        assert!(html.html.contains("<strong>world</strong>"), "{html:?}");
        assert!(
            html.html.contains("href=\"https://example.test/next/\""),
            "{html:?}"
        );
        assert!(!html.html.contains("Outside"));
        let original = page
            .html_fragment(&selection, &HtmlFragmentOptions::default(), &cancellation)
            .unwrap();
        assert!(original.html.contains("href=\"next/\""), "{original:?}");
        assert!(matches!(
            page.html_fragment(
                &HtmlFragmentSelection::Id("missing".into()),
                &HtmlFragmentOptions::default(),
                &cancellation
            ),
            Err(HtmlFragmentError::MissingId { .. })
        ));
        cancellation.cancel();
        assert!(
            page.html_fragment(&selection, &HtmlFragmentOptions::default(), &cancellation)
                .unwrap_err()
                .is_cancelled()
        );
    }

    #[test]
    fn frame_links_resolve_against_the_bundle() {
        use typst::foundations::{Label, Selector};
        use typst::layout::{Abs, Point, Size};
        let (_directory, world) = world(
            "#document(\"index.html\")[#link(<target>)[Next]]\n#document(\"other/index.html\")[= Target <target>]",
        );
        let cancellation = BundleCancellation::default();
        let compilation = crate::compile_bundle_world(&world, &cancellation).unwrap();
        let label = Label::new(typst::utils::PicoStr::intern("target")).unwrap();
        let location = compilation
            .introspector()
            .query_unique(&Selector::Label(label))
            .unwrap()
            .location()
            .unwrap();
        let source = VirtualPath::new("index.html").unwrap();
        let resolver = LateLinkResolver::new(Some(&source), compilation.introspector());
        let size = Size::new(Abs::pt(10.0), Abs::pt(10.0));
        let mut frame = Frame::soft(size);
        frame.push(
            Point::zero(),
            FrameItem::Link(Destination::Location(location), size),
        );
        let original = frame.clone();
        rewrite_frame(&mut frame, &resolver, &cancellation, &mut |uri, _| {
            Ok(format!("https://example.test/{uri}"))
        })
        .unwrap();
        let FrameItem::Link(Destination::Url(uri), _) = &frame.items().next().unwrap().1 else {
            panic!("expected a rewritten frame link");
        };
        assert!(
            uri.as_str()
                .starts_with("https://example.test/other/index.html#"),
            "{uri:?}"
        );
        assert!(
            matches!(&original.items().next().unwrap().1, FrameItem::Link(Destination::Location(retained), _) if *retained == location)
        );
    }

    #[test]
    fn frameless_destination_needs_no_uri() {
        use typst::introspection::{EmptyIntrospector, Location};
        use typst::layout::{Abs, Point, Size};
        let location = Location::new(17);
        let size = Size::new(Abs::pt(1.0), Abs::pt(1.0));
        let mut frame = Frame::soft(size);
        frame.push(
            Point::zero(),
            FrameItem::Link(Destination::Location(location), size),
        );
        rewrite_frame(
            &mut frame,
            &LateLinkResolver::new(None, &EmptyIntrospector),
            &BundleCancellation::default(),
            &mut |_, _| panic!("a frame destination without a URI does not invoke the URL policy"),
        )
        .unwrap();
        assert!(
            matches!(&frame.items().next().unwrap().1, FrameItem::Link(Destination::Location(retained), _) if *retained == location)
        );
    }

    #[test]
    fn template_content_stays_inert() {
        let (_directory, world) = world(
            r#"#document("index.html")[
  #html.p(id: "selected")[Active]
  #html.template[#html.p(id: "selected")[Inactive] #html.a(href: "https://[")[Deferred]]
]"#,
        );
        let cancellation = BundleCancellation::default();
        let compilation = crate::compile_bundle_world(&world, &cancellation).unwrap();
        let page = compilation.documents().next().unwrap();
        let selected = page
            .html_fragment(
                &HtmlFragmentSelection::Id("selected".into()),
                &HtmlFragmentOptions::default(),
                &cancellation,
            )
            .unwrap();
        assert!(selected.html.contains("Active"));
        assert!(!selected.html.contains("Inactive"));
        let body = page
            .html_fragment_with_links(
                &HtmlFragmentSelection::Body,
                &HtmlFragmentOptions::default(),
                &cancellation,
                &mut |_, _| {
                    Err(HtmlFragmentError::InvalidReference {
                        message: "inactive URL reached the policy".into(),
                    })
                },
            )
            .unwrap();
        assert!(body.html.contains("https://["), "{body:?}");
    }
}
