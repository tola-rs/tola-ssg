//! Text payloads of compiled raw-text elements.

use typst::syntax::{Span, VirtualPath};
use typst_html::{HtmlDocument, HtmlElement, HtmlNode};

use crate::bundle::BundleCancellation;

/// A compiled element whose children are text rather than markup.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HtmlRawText {
    /// The stylesheet text of a `<style>` element.
    Stylesheet,
    /// The source text of a classic `<script>` element.
    Script,
    /// The source text of a `<script type="module">` element.
    Module,
}

/// A raw-text rewrite was cancelled before any document changed.
#[derive(Debug, thiserror::Error)]
pub enum HtmlRawTextError {
    /// The caller cancelled traversal or preparation.
    #[error("HTML raw-text rewriting was cancelled")]
    Cancelled,
}

impl crate::diagnostic::CancelledError for HtmlRawTextError {
    fn from_cancellation() -> Self {
        Self::Cancelled
    }
}

/// One pending payload replacement.
pub(crate) struct RawTextChange {
    /// Child indices from the document root to the owning element.
    path: Vec<usize>,
    /// The replacement payload.
    text: String,
    /// The span the replacement text reports.
    span: Span,
}

/// A `<script>` whose `type` names neither JavaScript nor a module is a data block; its payload is
/// not source text, so it is not offered.
fn raw_text_kind(element: &HtmlElement) -> Option<HtmlRawText> {
    let tag = element.tag.resolve();
    let tag = tag.as_str();
    if tag.eq_ignore_ascii_case("style") {
        return Some(HtmlRawText::Stylesheet);
    }
    if !tag.eq_ignore_ascii_case("script") {
        return None;
    }
    let declared = element.attrs.0.iter().find_map(|(name, value)| {
        name.resolve()
            .as_str()
            .eq_ignore_ascii_case("type")
            .then_some(value.as_str())
    });
    match declared {
        None => Some(HtmlRawText::Script),
        Some(declared) => script_kind(declared),
    }
}

fn script_kind(declared: &str) -> Option<HtmlRawText> {
    // The type attribute is a MIME type; parameters follow a `;` separator.
    let essence = declared.split(';').next().unwrap_or_default().trim();
    if essence.is_empty() {
        return Some(HtmlRawText::Script);
    }
    if essence.eq_ignore_ascii_case("module") {
        return Some(HtmlRawText::Module);
    }
    is_javascript_mime_type(essence).then_some(HtmlRawText::Script)
}

/// Whether a MIME essence names JavaScript.
///
/// See the HTML specification's JavaScript MIME type table.
fn is_javascript_mime_type(essence: &str) -> bool {
    matches!(
        essence.to_ascii_lowercase().as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

/// The text payload of one element, when every child is text or an introspection tag.
fn payload(element: &HtmlElement) -> Option<String> {
    let mut text = String::new();
    for child in &element.children {
        match child {
            HtmlNode::Text(piece, _) => text.push_str(piece),
            HtmlNode::Tag(_) => {}
            HtmlNode::Element(_) | HtmlNode::Frame(_) => return None,
        }
    }
    Some(text)
}

/// The span the payload reports: the text it was written from.
fn payload_span(element: &HtmlElement) -> Span {
    element
        .children
        .iter()
        .find_map(|child| match child {
            HtmlNode::Text(_, span) => Some(*span),
            _ => None,
        })
        .unwrap_or_else(Span::detached)
}

/// Collect the payload replacements one document's rewrite requests.
///
/// The callback observes the document path, the payload kind, the span the payload reports, and the
/// current text; `None` keeps the payload unchanged.
pub(crate) fn collect_raw_text(
    document: &HtmlDocument,
    path: &VirtualPath,
    cancellation: &BundleCancellation,
    rewrite: &mut dyn FnMut(&VirtualPath, HtmlRawText, Span, &str) -> Option<String>,
) -> Result<Vec<RawTextChange>, HtmlRawTextError> {
    let mut changes = Vec::new();
    visit(
        document.root(),
        path,
        &mut Vec::new(),
        cancellation,
        rewrite,
        &mut changes,
    )?;
    Ok(changes)
}

fn visit(
    element: &HtmlElement,
    document: &VirtualPath,
    path: &mut Vec<usize>,
    cancellation: &BundleCancellation,
    rewrite: &mut dyn FnMut(&VirtualPath, HtmlRawText, Span, &str) -> Option<String>,
    changes: &mut Vec<RawTextChange>,
) -> Result<(), HtmlRawTextError> {
    cancellation.ensure_active_as()?;
    if let Some(kind) = raw_text_kind(element)
        && let Some(text) = payload(element)
    {
        let span = payload_span(element);
        if let Some(replacement) = rewrite(document, kind, span, &text)
            && replacement != text
        {
            changes.push(RawTextChange {
                path: path.clone(),
                text: replacement,
                span,
            });
        }
    }
    for (position, child) in element.children.iter().enumerate() {
        cancellation.ensure_active_as()?;
        let HtmlNode::Element(child) = child else {
            continue;
        };
        path.push(position);
        visit(child, document, path, cancellation, rewrite, changes)?;
        path.pop();
    }
    cancellation.ensure_active_as()
}

/// Install collected payload replacements into one document copy.
pub(crate) fn apply_raw_text(
    document: &mut HtmlDocument,
    changes: Vec<RawTextChange>,
    cancellation: &BundleCancellation,
) -> Result<(), HtmlRawTextError> {
    for change in changes {
        cancellation.ensure_active_as()?;
        let mut element = document.root_mut();
        for &index in &change.path {
            let HtmlNode::Element(child) = &mut element.children.make_mut()[index] else {
                unreachable!("a raw-text change preserves the indexed element path")
            };
            element = child;
        }
        let mut rebuilt: Vec<HtmlNode> = Vec::with_capacity(element.children.len());
        let mut replaced = false;
        for child in element.children.iter() {
            match child {
                // A raw-text element's text is one payload; introspection tags stay in place.
                HtmlNode::Text(..) if !replaced => {
                    replaced = true;
                    rebuilt.push(HtmlNode::Text(change.text.as_str().into(), change.span));
                }
                HtmlNode::Text(..) => {}
                other => rebuilt.push(other.clone()),
            }
        }
        element.children = rebuilt.into();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst_html::{HtmlAttr, HtmlTag};

    fn element(tag: &str, attrs: &[(&str, &str)]) -> HtmlElement {
        let mut element = HtmlElement::new(HtmlTag::intern(tag).unwrap());
        for (name, value) in attrs {
            element.attrs.push(HtmlAttr::intern(name).unwrap(), *value);
        }
        element
    }

    fn with_text(element: HtmlElement, text: &str) -> HtmlElement {
        element.with_children(
            [HtmlNode::text(text, Span::detached())]
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn raw_text_element_kinds() {
        assert_eq!(
            raw_text_kind(&element("style", &[])),
            Some(HtmlRawText::Stylesheet)
        );
        assert_eq!(
            raw_text_kind(&element("STYLE", &[])),
            Some(HtmlRawText::Stylesheet)
        );
        assert_eq!(
            raw_text_kind(&element("script", &[])),
            Some(HtmlRawText::Script)
        );
        assert_eq!(
            raw_text_kind(&element("script", &[("type", "module")])),
            Some(HtmlRawText::Module)
        );
        assert_eq!(
            raw_text_kind(&element("script", &[("type", "text/JavaScript")])),
            Some(HtmlRawText::Script)
        );
        assert_eq!(
            raw_text_kind(&element(
                "script",
                &[("type", "text/javascript; charset=utf-8")]
            )),
            Some(HtmlRawText::Script)
        );
    }

    #[test]
    fn data_blocks_have_no_payload() {
        for (tag, attrs) in [
            ("script", &[("type", "application/json")][..]),
            ("script", &[("type", "importmap")][..]),
            ("script", &[("type", "speculationrules")][..]),
            ("title", &[][..]),
            ("pre", &[][..]),
            ("div", &[][..]),
        ] {
            assert_eq!(raw_text_kind(&element(tag, attrs)), None, "{tag} {attrs:?}");
        }
    }

    #[test]
    fn text_children_form_one_payload() {
        let element = element("style", &[]).with_children(
            [
                HtmlNode::text(".a {", Span::detached()),
                HtmlNode::text("color: red }", Span::detached()),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(payload(&element).as_deref(), Some(".a {color: red }"));
    }

    #[test]
    fn element_children_suppress_the_payload() {
        let element = with_text(element("script", &[]), "first()").with_children(
            [
                HtmlNode::text("first()", Span::detached()),
                HtmlNode::Element(with_text(element("span", &[]), "nested")),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(payload(&element), None);
    }
}
