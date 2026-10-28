//! Structured observations from Typst's final HTML DOM.

use typst::introspection::Introspector;
use typst::layout::{Frame, FrameItem};
use typst::model::{Destination, LateLinkResolver};
use typst::syntax::Span;
use typst::syntax::VirtualPath;
use typst_html::{HtmlElement, HtmlFrame, HtmlNode};

use crate::bundle::BundleCancellation;
use crate::diagnostic::CompileError;

use super::reference::{
    HtmlReferenceUse, RefreshDestination, ascii_lowercase, attribute, classify_reference_use,
    refresh_destination, unique_attributes, url_ranges,
};

/// Syntax that introduced an HTML fragment target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HtmlFragmentKind {
    /// An `id` attribute.
    Id,
    /// A legacy `<a name>` anchor.
    AnchorName,
    /// An anchor emitted inside an HTML frame's SVG island.
    FrameAnchor,
}

/// One final DOM fragment target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlFragment {
    value: String,
    kind: HtmlFragmentKind,
    span: Span,
}

impl HtmlFragment {
    /// Decoded fragment identifier as emitted by Typst.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Syntax that introduced the target.
    pub fn kind(&self) -> HtmlFragmentKind {
        self.kind
    }

    /// Element- or frame-level Typst source span.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// One URL-bearing HTML attribute occurrence in final DOM order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlReference {
    tag: String,
    attribute: String,
    destination: String,
    attribute_value: String,
    relation: Option<String>,
    reference_use: HtmlReferenceUse,
    span: Span,
}

impl HtmlReference {
    /// Final HTML element tag without angle brackets.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// URL-bearing attribute name.
    pub fn attribute(&self) -> &str {
        &self.attribute
    }

    /// One destination extracted from the attribute.
    ///
    /// This differs from [`Self::attribute_value`] for `srcset` and refresh
    /// metadata, which can encode a destination inside a larger value.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Complete attribute value as represented in Typst's final DOM.
    pub fn attribute_value(&self) -> &str {
        &self.attribute_value
    }

    /// The element's `rel` attribute when present.
    pub fn relation(&self) -> Option<&str> {
        self.relation.as_deref()
    }

    /// Syntax-implied browser use; this does not assert the resolved output kind.
    pub fn reference_use(&self) -> HtmlReferenceUse {
        self.reference_use
    }

    /// Typst source span of the final DOM element.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// First `<base href>` occurrence observed in a final HTML document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlBaseHref {
    value: String,
    span: Span,
}

impl HtmlBaseHref {
    /// Raw final-DOM href value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Typst source span of the `<base>` element.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// Deterministic structural observations from one final Typst HTML document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HtmlDocumentInventory {
    fragments: Vec<HtmlFragment>,
    references: Vec<HtmlReference>,
    base_hrefs: Vec<HtmlBaseHref>,
}

impl HtmlDocumentInventory {
    /// Traverse one final Typst HTML document without serializing or reparsing it.
    pub fn from_document(
        document: &typst_html::HtmlDocument,
        cancellation: &BundleCancellation,
    ) -> Result<Self, CompileError> {
        Self::from_root(
            document.root(),
            LateLinkResolver::new(None, document.introspector().as_ref()),
            cancellation,
        )
    }

    pub(crate) fn from_bundle_document(
        path: &VirtualPath,
        document: &typst_html::HtmlDocument,
        introspector: &dyn Introspector,
        cancellation: &BundleCancellation,
    ) -> Result<Self, CompileError> {
        Self::from_root(
            document.root(),
            LateLinkResolver::new(Some(path), introspector),
            cancellation,
        )
    }

    fn from_root(
        root: &HtmlElement,
        link_resolver: LateLinkResolver<'_>,
        cancellation: &BundleCancellation,
    ) -> Result<Self, CompileError> {
        let mut builder = InventoryBuilder {
            inventory: Self::default(),
            link_resolver,
            refresh_seen: false,
            cancellation,
        };
        builder.visit_element(root)?;
        cancellation.ensure_active()?;
        Ok(builder.inventory)
    }

    /// Final DOM fragment targets in document order.
    pub fn fragments(&self) -> &[HtmlFragment] {
        &self.fragments
    }

    /// URL-bearing attribute occurrences in document order.
    pub fn references(&self) -> &[HtmlReference] {
        &self.references
    }

    /// First `<base href>` occurrence, if any.
    pub fn base_href(&self) -> Option<&HtmlBaseHref> {
        self.base_hrefs.first()
    }

    /// Every `<base href>` occurrence in document order.
    pub fn base_hrefs(&self) -> &[HtmlBaseHref] {
        &self.base_hrefs
    }
}

struct InventoryBuilder<'a> {
    inventory: HtmlDocumentInventory,
    link_resolver: LateLinkResolver<'a>,
    refresh_seen: bool,
    cancellation: &'a BundleCancellation,
}

impl InventoryBuilder<'_> {
    fn visit_element(&mut self, element: &HtmlElement) -> Result<(), CompileError> {
        self.cancellation.ensure_active()?;
        let resolved_tag = element.tag.resolve();
        let tag = ascii_lowercase(resolved_tag.as_str());
        let relation = attribute(element, "rel").map(str::to_owned);

        let id = attribute(element, "id").filter(|id| !id.is_empty());
        if let Some(id) = id {
            self.inventory.fragments.push(HtmlFragment {
                value: id.to_owned(),
                kind: HtmlFragmentKind::Id,
                span: element.span,
            });
        }
        if tag == "a"
            && let Some(name) = attribute(element, "name")
            && !name.is_empty()
            && Some(name) != id
        {
            self.inventory.fragments.push(HtmlFragment {
                value: name.to_owned(),
                kind: HtmlFragmentKind::AnchorName,
                span: element.span,
            });
        }

        if tag == "base"
            && let Some(value) = attribute(element, "href")
        {
            self.inventory.base_hrefs.push(HtmlBaseHref {
                value: value.to_owned(),
                span: element.span,
            });
        }

        for attribute in unique_attributes(element) {
            self.cancellation.ensure_active()?;
            let name = attribute.name.as_str();
            let Some(reference_use) = classify_reference_use(element, &tag, name) else {
                continue;
            };
            if name == "ping" || name == "srcset" || name == "imagesrcset" {
                for range in url_ranges(&tag, name, attribute.value) {
                    self.cancellation.ensure_active()?;
                    self.inventory.references.push(reference(
                        &tag,
                        element,
                        name,
                        attribute.value,
                        attribute.value[range].to_owned(),
                        relation.as_deref(),
                        reference_use,
                    ));
                }
            } else if tag == "meta" && name == "content" {
                if self.refresh_seen {
                    continue;
                }
                let RefreshDestination::Declared(destination) =
                    refresh_destination(attribute.value)
                else {
                    continue;
                };
                self.refresh_seen = true;
                let Some(destination) = destination else {
                    continue;
                };
                self.inventory.references.push(reference(
                    &tag,
                    element,
                    name,
                    attribute.value,
                    destination.to_owned(),
                    relation.as_deref(),
                    reference_use,
                ));
            } else {
                self.inventory.references.push(reference(
                    &tag,
                    element,
                    name,
                    attribute.value,
                    attribute.value.to_string(),
                    relation.as_deref(),
                    reference_use,
                ));
            }
        }

        if tag == "template" {
            return Ok(());
        }

        for child in &element.children {
            self.cancellation.ensure_active()?;
            match child {
                HtmlNode::Element(child) => self.visit_element(child)?,
                HtmlNode::Frame(frame) => self.visit_frame(frame)?,
                HtmlNode::Tag(_) | HtmlNode::Text(..) => {}
            }
        }
        Ok(())
    }

    fn visit_frame(&mut self, frame: &HtmlFrame) -> Result<(), CompileError> {
        self.cancellation.ensure_active()?;
        if let Some(id) = &frame.id
            && !id.is_empty()
        {
            self.inventory.fragments.push(HtmlFragment {
                value: id.to_string(),
                kind: HtmlFragmentKind::Id,
                span: frame.span,
            });
        }
        for (_, id) in &frame.anchors {
            self.cancellation.ensure_active()?;
            if !id.is_empty() {
                self.inventory.fragments.push(HtmlFragment {
                    value: id.to_string(),
                    kind: HtmlFragmentKind::FrameAnchor,
                    span: frame.span,
                });
            }
        }
        self.visit_frame_items(&frame.inner, frame.span)
    }

    fn visit_frame_items(&mut self, frame: &Frame, span: Span) -> Result<(), CompileError> {
        for (_, item) in frame.items() {
            self.cancellation.ensure_active()?;
            match item {
                FrameItem::Group(group) => self.visit_frame_items(&group.frame, span)?,
                FrameItem::Link(destination, _) => {
                    let destination = match destination {
                        Destination::Url(url) => Some(url.as_str().to_owned()),
                        Destination::Location(location) => self
                            .link_resolver
                            .resolve(*location)
                            .and_then(|resolved| resolved.into_relative_uri().ok())
                            .map(|uri| uri.to_string()),
                        Destination::Position(_) => None,
                    };
                    if let Some(destination) = destination {
                        self.inventory.references.push(HtmlReference {
                            tag: "svg:a".into(),
                            attribute: "href".into(),
                            attribute_value: destination.clone(),
                            destination,
                            relation: None,
                            reference_use: HtmlReferenceUse::Navigation,
                            span,
                        });
                    }
                }
                FrameItem::Text(_)
                | FrameItem::Shape(..)
                | FrameItem::Image(..)
                | FrameItem::Tag(_) => {}
            }
        }
        Ok(())
    }
}

fn reference(
    tag: &str,
    element: &HtmlElement,
    attribute: &str,
    attribute_value: &str,
    destination: String,
    relation: Option<&str>,
    reference_use: HtmlReferenceUse,
) -> HtmlReference {
    HtmlReference {
        tag: tag.to_owned(),
        attribute: attribute.to_owned(),
        destination,
        attribute_value: attribute_value.to_owned(),
        relation: relation.map(str::to_owned),
        reference_use,
        span: element.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst_html::{HtmlElement, HtmlNode, attr, tag};

    fn inventory_of(root: &HtmlElement) -> HtmlDocumentInventory {
        HtmlDocumentInventory::from_root(
            root,
            LateLinkResolver::new(None, &typst::introspection::EmptyIntrospector),
            &BundleCancellation::default(),
        )
        .unwrap()
    }

    /// Destination, use, and attribute value of every collected reference.
    fn references_of(inventory: &HtmlDocumentInventory) -> Vec<(&str, HtmlReferenceUse, &str)> {
        inventory
            .references()
            .iter()
            .map(|reference| {
                (
                    reference.destination(),
                    reference.reference_use(),
                    reference.attribute_value(),
                )
            })
            .collect()
    }

    #[test]
    fn inventory_keeps_dom_order() {
        let mut root = HtmlElement::new(tag::html).with_attr(attr::id, "root");
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::base).with_attr(attr::href, "/docs/"),
        ));
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::a).with_attr(attr::href, "/guide/#intro"),
        ));
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::img)
                .with_attr(attr::id, "diagram")
                .with_attr(attr::srcset, "/one.png 1x, /two.png 2x"),
        ));
        let inventory = inventory_of(&root);

        assert_eq!(
            inventory
                .fragments()
                .iter()
                .map(HtmlFragment::value)
                .collect::<Vec<_>>(),
            ["root", "diagram"]
        );
        assert_eq!(inventory.base_href().unwrap().value(), "/docs/");
        assert_eq!(
            references_of(&inventory),
            [
                (
                    "/guide/#intro",
                    HtmlReferenceUse::Navigation,
                    "/guide/#intro",
                ),
                (
                    "/one.png",
                    HtmlReferenceUse::GenericResource,
                    "/one.png 1x, /two.png 2x",
                ),
                (
                    "/two.png",
                    HtmlReferenceUse::GenericResource,
                    "/one.png 1x, /two.png 2x",
                ),
            ]
        );
    }

    #[test]
    fn matching_anchor_is_one_target() {
        let root = HtmlElement::new(tag::a)
            .with_attr(attr::id, "same")
            .with_attr(typst_html::attr::name, "same");
        let inventory = inventory_of(&root);

        assert_eq!(inventory.fragments().len(), 1);
        assert_eq!(inventory.fragments()[0].value(), "same");
        assert_eq!(inventory.fragments()[0].kind(), HtmlFragmentKind::Id);
    }

    #[test]
    fn form_controls_decide_their_urls() {
        let mut root = HtmlElement::new(tag::html);
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::input)
                .with_attr(attr::r#type, "text")
                .with_attr(attr::formaction, "/ignored-input/"),
        ));
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::input)
                .with_attr(attr::r#type, "submit")
                .with_attr(attr::formaction, "/submitted/"),
        ));
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::button)
                .with_attr(attr::r#type, "reset")
                .with_attr(attr::formaction, "/ignored-button/"),
        ));
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::button).with_attr(attr::formaction, "/button-default/"),
        ));
        root.children.push(HtmlNode::Element(
            HtmlElement::new(tag::a).with_attr(attr::ping, "/first-ping /second-ping"),
        ));
        let inventory = inventory_of(&root);

        assert_eq!(
            inventory
                .references()
                .iter()
                .map(|reference| (
                    reference.destination(),
                    reference.reference_use(),
                    reference.attribute()
                ))
                .collect::<Vec<_>>(),
            [
                ("/submitted/", HtmlReferenceUse::Navigation, "formaction"),
                (
                    "/button-default/",
                    HtmlReferenceUse::Navigation,
                    "formaction"
                ),
                ("/first-ping", HtmlReferenceUse::GenericResource, "ping"),
                ("/second-ping", HtmlReferenceUse::GenericResource, "ping"),
            ]
        );
    }
}
