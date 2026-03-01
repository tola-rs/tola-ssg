//! Typst HtmlDocument to tola-vdom Raw VDOM conversion
//!
//! Converts typst-html output to `Document<TolaSite::Raw>` for unified
//! processing with other formats (e.g., Markdown).
//!
//! # Flow
//!
//! ```text
//! typst_batch::HtmlDocument
//!         │
//! Document<TolaSite::Raw>  (Frames rendered as SVG in parallel)
//!         │
//! Document<Indexed>
//! ```
//!
//! # Performance
//!
//! SVG rendering (for CeTZ plots, math, etc.) is parallelized via typst-batch.
//! The conversion uses a two-phase approach:
//!
//! - **Collection**: Traverse DOM, collect all frames with sequential IDs
//! - **Render**: Batch render frames to SVG (parallel when batch feature enabled)
//! - **Build**: Construct VDOM using pre-rendered SVGs

use smallvec::SmallVec;
use tola_vdom::prelude::*;
use typst_batch::prelude::*;

use crate::compiler::family::TolaSite;
use crate::utils::html::{is_raw_text_element, parse_attributes};

// =============================================================================
// Frame Collection (Phase 1)
// =============================================================================

type FrameId = u32;

/// Collect all frames from the document in document order
fn collect_frames<'a>(doc: &'a HtmlDocument) -> Vec<HtmlFrame<'a>> {
    let mut frames = Vec::new();
    collect_frames_from_element(doc.root(), &mut frames);
    frames
}

fn collect_frames_from_element<'a>(elem: HtmlElement<'a>, frames: &mut Vec<HtmlFrame<'a>>) {
    for child in elem.children() {
        match child.kind() {
            NodeKind::Frame(frame) => frames.push(frame),
            NodeKind::Element(child_elem) => collect_frames_from_element(child_elem, frames),
            _ => {}
        }
    }
}

// =============================================================================
// VDOM Construction (Phase 2)
// =============================================================================

struct Converter<'a> {
    doc: &'a HtmlDocument,
    svg_cache: Vec<String>,
    next_frame_id: FrameId,
    baseline_align: bool,
}

const MAX_ABS_VERTICAL_ALIGN_EM: f64 = 3.0;

#[inline]
fn normalize_vertical_align_em(vertical_align_em: f64) -> Option<f64> {
    if !vertical_align_em.is_finite() {
        return None;
    }

    if vertical_align_em.abs() > MAX_ABS_VERTICAL_ALIGN_EM {
        return None;
    }

    Some(vertical_align_em)
}

impl<'a> Converter<'a> {
    fn new(doc: &'a HtmlDocument, svg_cache: Vec<String>, baseline_align: bool) -> Self {
        Self {
            doc,
            svg_cache,
            next_frame_id: 0,
            baseline_align,
        }
    }

    fn convert_document(&mut self) -> Document<TolaSite::Raw> {
        let root = self.convert_element(self.doc.root());
        Document::new(root)
    }

    fn convert_element(&mut self, elem: HtmlElement<'_>) -> Element<TolaSite::Raw> {
        let tag = elem.tag();
        let is_raw_text = is_raw_text_element(&tag);

        let children: SmallVec<[Node<TolaSite::Raw>; 8]> = elem
            .children()
            .filter_map(|child| self.convert_node(child, is_raw_text))
            .collect();

        let attrs = Attrs::from_iter(elem.attrs().map(|(k, v)| (k.into(), v.into())));
        let mut element = TolaSite::element(&tag, attrs);
        element.children = children;
        element
    }

    fn convert_node(
        &mut self,
        node: typst_batch::html::HtmlNode<'_>,
        in_raw_text: bool,
    ) -> Option<Node<TolaSite::Raw>> {
        match node.kind() {
            NodeKind::Tag => None,
            NodeKind::Text(text) => {
                let text_node = if in_raw_text {
                    Text::raw(text.to_string())
                } else {
                    Text::new(text.to_string())
                };
                Some(Node::Text(text_node))
            }
            NodeKind::Element(elem) => Some(Node::Element(Box::new(self.convert_element(elem)))),
            NodeKind::Frame(frame) => {
                let id = self.next_frame_id as usize;
                self.next_frame_id += 1;

                // Typst's built-in baseline formula:
                // vertical-align = -(height - baseline) / text_size
                let vertical_align_em = if self.baseline_align {
                    normalize_vertical_align_em(frame.vertical_align_em())
                } else {
                    None
                };

                self.svg_cache
                    .get(id)
                    .map(|svg| svg_string_to_node(svg, vertical_align_em))
            }
        }
    }
}

// =============================================================================
// SVG Parsing
// =============================================================================

/// Convert an SVG string to a VDOM node with vertical-align for baseline alignment
fn svg_string_to_node(svg: &str, vertical_align_em: Option<f64>) -> Node<TolaSite::Raw> {
    let (mut attrs, inner_content) = parse_svg_string(svg);

    // Add vertical-align to style attribute for baseline alignment
    if let Some(vertical_align_em) = vertical_align_em {
        let existing_style = attrs
            .iter()
            .find(|(k, _)| k == "style")
            .map(|(_, v)| v.clone());

        let new_style = match existing_style {
            Some(s) if !s.is_empty() => {
                format!("{}; vertical-align: {:.4}em", s, vertical_align_em)
            }
            _ => format!("vertical-align: {:.4}em", vertical_align_em),
        };

        attrs.retain(|(k, _)| k != "style");
        attrs.push(("style".to_string(), new_style));
    }

    let attrs = Attrs::from_iter(attrs.into_iter().map(|(k, v)| (k.into(), v.into())));
    let mut svg_elem = TolaSite::element("svg", attrs);

    if !inner_content.is_empty() {
        svg_elem.children = SmallVec::from_vec(vec![Node::Text(Text::raw(inner_content))]);
    }

    Node::Element(Box::new(svg_elem))
}

/// Parse an SVG string to extract attributes and inner content
///
/// Input: `<svg viewBox="0 0 100 100" class="foo">inner content</svg>`
/// Output: (vec![("viewBox", "0 0 100 100"), ("class", "foo")], "inner content")
fn parse_svg_string(svg: &str) -> (Vec<(String, String)>, String) {
    // Find the opening tag end
    let Some(tag_start) = svg.find('<') else {
        return (vec![], svg.to_string());
    };

    let Some(tag_end) = svg[tag_start..].find('>') else {
        return (vec![], svg.to_string());
    };
    let tag_end = tag_start + tag_end;

    // Check if it's self-closing
    let is_self_closing = svg[..tag_end].ends_with('/');

    // Extract opening tag content: "svg viewBox="0 0 100 100" ..."
    let tag_content = &svg[tag_start + 1..if is_self_closing {
        tag_end - 1
    } else {
        tag_end
    }];
    let tag_content = tag_content.trim();

    // Skip "svg" tag name
    let attr_start = tag_content
        .find(char::is_whitespace)
        .unwrap_or(tag_content.len());
    let attr_str = &tag_content[attr_start..].trim();

    // Parse attributes
    let attrs = parse_attributes(attr_str);

    // Extract inner content (between > and </svg>)
    let inner_content = if is_self_closing {
        String::new()
    } else {
        let content_start = tag_end + 1;
        let content_end = svg.rfind("</svg>").unwrap_or(svg.len());
        svg[content_start..content_end].to_string()
    };

    (attrs, inner_content)
}

// =============================================================================
// Public API
// =============================================================================

/// Convert typst HtmlDocument to Raw VDOM
///
/// Uses three phases for optimal performance:
/// 1. Collect all frames from the document
/// 2. Batch render frames to SVG (parallel when batch feature enabled)
/// 3. Build VDOM tree with pre-rendered SVGs
///
/// # Arguments
/// * `doc` - The Typst HtmlDocument to convert
/// * `baseline_align` - Whether to apply vertical-align for baseline alignment
pub fn from_typst_html(doc: &HtmlDocument, baseline_align: bool) -> Document<TolaSite::Raw> {
    // Collect all frames
    let frames = collect_frames(doc);

    // Batch render (parallel when available)
    let svg_cache = doc.render_frames(&frames);

    // Build VDOM
    let mut converter = Converter::new(doc, svg_cache, baseline_align);
    converter.convert_document()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identify_family() {
        // Test using TolaSite::identify()
        assert_eq!(TolaSite::identify("a", &Attrs::new()), "link");
        assert_eq!(TolaSite::identify("h1", &Attrs::new()), "heading");
        assert_eq!(TolaSite::identify("svg", &Attrs::new()), "svg");
        assert_eq!(TolaSite::identify("img", &Attrs::new()), "media");
        assert_eq!(TolaSite::identify("video", &Attrs::new()), "media");
        assert_eq!(TolaSite::identify("audio", &Attrs::new()), "media");

        // Generic elements go to "none"
        assert_eq!(TolaSite::identify("p", &Attrs::new()), "none");
        assert_eq!(TolaSite::identify("code", &Attrs::new()), "none");
        assert_eq!(TolaSite::identify("pre", &Attrs::new()), "none");
        assert_eq!(TolaSite::identify("div", &Attrs::new()), "none");
        assert_eq!(TolaSite::identify("span", &Attrs::new()), "none");
    }

    #[test]
    fn test_parse_svg_string_basic() {
        let svg = r#"<svg viewBox="0 0 100 100" class="test">inner content</svg>"#;
        let (attrs, inner) = parse_svg_string(svg);

        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0], ("viewBox".to_string(), "0 0 100 100".to_string()));
        assert_eq!(attrs[1], ("class".to_string(), "test".to_string()));
        assert_eq!(inner, "inner content");
    }

    #[test]
    fn test_parse_svg_string_complex() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 595.28 841.89"><g>paths...</g></svg>"#;
        let (attrs, inner) = parse_svg_string(svg);

        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].0, "xmlns");
        assert_eq!(attrs[1].0, "viewBox");
        assert!(inner.contains("<g>"));
    }

    #[test]
    fn test_parse_svg_string_self_closing() {
        let svg = r#"<svg viewBox="0 0 10 10"/>"#;
        let (attrs, inner) = parse_svg_string(svg);

        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].0, "viewBox");
        assert!(inner.is_empty());
    }

    #[test]
    fn test_normalize_vertical_align_em() {
        assert_eq!(normalize_vertical_align_em(-0.4321), Some(-0.4321));
        assert_eq!(normalize_vertical_align_em(f64::NAN), None);
        assert_eq!(normalize_vertical_align_em(f64::INFINITY), None);
        assert_eq!(normalize_vertical_align_em(4.0), None);
        assert_eq!(normalize_vertical_align_em(-3.5), None);
    }

    #[test]
    fn test_svg_string_to_node_injects_vertical_align_style() {
        let svg = r#"<svg viewBox="0 0 10 10"></svg>"#;
        let node = svg_string_to_node(svg, Some(-0.5));

        let Node::Element(svg_elem) = node else {
            panic!("expected element node");
        };

        assert_eq!(
            svg_elem.get_attr("style"),
            Some("vertical-align: -0.5000em")
        );
    }

    #[test]
    fn test_svg_string_to_node_appends_vertical_align_style() {
        let svg = r#"<svg style="overflow: visible"></svg>"#;
        let node = svg_string_to_node(svg, Some(-0.25));

        let Node::Element(svg_elem) = node else {
            panic!("expected element node");
        };

        assert_eq!(
            svg_elem.get_attr("style"),
            Some("overflow: visible; vertical-align: -0.2500em")
        );
    }

    #[test]
    fn test_svg_string_to_node_skips_vertical_align_when_none() {
        let svg = r#"<svg viewBox="0 0 10 10"></svg>"#;
        let node = svg_string_to_node(svg, None);

        let Node::Element(svg_elem) = node else {
            panic!("expected element node");
        };

        assert_eq!(svg_elem.get_attr("style"), None);
    }
}
