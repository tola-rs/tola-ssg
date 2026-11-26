//! Frozen SVG icon values and local-reference prefixing.

use std::{
    fmt::{self, Write},
    sync::Arc,
};

mod document;
mod geometry;

use document::ReferenceSpan;
pub(crate) use document::SvgDocument;
pub use geometry::ViewBox;

const MAX_SVG_BYTES: usize = 4 * 1024 * 1024;
const MAX_SVG_ID_BYTES: usize = 512;

struct InlineSize(usize);

impl Write for InlineSize {
    fn write_str(&mut self, val: &str) -> fmt::Result {
        self.0 = self
            .0
            .checked_add(val.len())
            .filter(|size| *size <= MAX_SVG_BYTES)
            .ok_or(fmt::Error)?;
        Ok(())
    }
}

/// Paint sources present in an icon's rendered content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconPaint {
    /// All visible paints follow inherited `currentColor`, allowing a color-following mask.
    CurrentColor,
    /// All visible paints are fixed colors (including default black), requiring image rendering.
    Fixed,
    /// Paints mix `currentColor` and fixed colors, or effects prevent masking.
    Mixed,
}

/// Error returned when parsing or prefixing an SVG icon.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidSvg {
    /// Source, tree, or serialized output exceeds the size limit.
    #[error("this SVG is larger than the {limit}-byte icon size limit")]
    Limit {
        /// Maximum allowed byte or node limit.
        limit: usize,
    },
    /// Input bytes are not valid UTF-8.
    #[error("this SVG is not UTF-8 text")]
    Utf8,
    /// XML parsing failed.
    #[error("this SVG is not valid XML at line {line}, column {column}")]
    Xml {
        /// One-based source line number.
        line: u32,
        /// One-based source column number.
        column: u32,
    },
    /// Root `<svg>` element or namespace is missing or invalid.
    #[error("this SVG does not have an <svg> root element in the SVG namespace")]
    Root,
    /// Root geometry lacks a valid finite positive `viewBox` or absolute dimensions.
    #[error("this SVG needs a viewBox or positive width and height in absolute units")]
    Geometry,
    /// Element is disallowed in a static self-contained icon.
    #[error("`{element}` cannot be used in an icon; use a plain shape element instead")]
    Element {
        /// Unsupported element tag name.
        element: String,
    },
    /// Attribute or inline style property is disallowed on the element.
    #[error("`{attribute}` is not supported on `{element}` in an icon")]
    Attribute {
        /// Containing element tag name.
        element: String,
        /// Unsupported attribute or style property name.
        attribute: String,
    },
    /// Attribute or inline style property has an invalid value.
    #[error("`{attribute}` on `{element}` has a value an icon cannot use")]
    Value {
        /// Containing element tag name.
        element: String,
        /// Attribute or style property name with the invalid value.
        attribute: String,
    },
    /// Path `d` attribute is not valid SVG path syntax.
    #[error("the `d` attribute of this path is not valid SVG path syntax")]
    Path,
    /// Resource reference points outside the local icon document.
    #[error("every `href` or `url()` in an icon must point at a `#id` in the same file")]
    ExternalReference,
    /// Two elements share the same `id`.
    #[error("two elements in this SVG use the id `{id}`")]
    DuplicateId {
        /// Duplicated element identifier.
        id: String,
    },
    /// Local reference names an `id` not defined in the document.
    #[error("this SVG references `{id}`, which the file does not define")]
    MissingReference {
        /// Unresolved local identifier.
        id: String,
    },
    /// Local reference targets an element of an incompatible SVG kind.
    #[error("`{attribute}` cannot reference `{id}`; the target is a different kind of element")]
    ReferenceTarget {
        /// Referencing attribute name.
        attribute: String,
        /// Referenced target identifier.
        id: String,
    },
    /// Containment and reference edges form a cycle.
    #[error("this SVG's references form a cycle")]
    ReferenceCycle,
    /// Caller-supplied ID prefix violates the icon-name grammar.
    #[error("the SVG id prefix is invalid: {0}")]
    InvalidIdPrefix(#[source] crate::InvalidIconName),
    /// Prefixed identifier exceeds the inline identifier byte limit.
    #[error(
        "an id in this icon would occupy {byte_len} bytes after prefixing; the limit is {MAX_SVG_ID_BYTES}"
    )]
    PrefixedIdTooLong {
        /// Byte length of the length-delimited prefix plus identifier.
        byte_len: usize,
    },
}

impl InvalidSvg {
    pub(crate) fn limit() -> Self {
        Self::Limit {
            limit: MAX_SVG_BYTES,
        }
    }
}

#[derive(Debug)]
struct ValidatedSvg {
    svg: Box<str>,
    vb: ViewBox,
    aspect_ratio: f64,
    paint: IconPaint,
    longest_id: usize,
    refs: Box<[ReferenceSpan]>,
}

/// Immutable, validated, self-contained SVG document backed by shared storage.
#[derive(Clone, Debug)]
pub struct SvgIcon(Arc<ValidatedSvg>);

impl SvgIcon {
    /// Parse standalone SVG bytes, rejecting malformed or unsupported content.
    pub fn parse(bytes: impl AsRef<[u8]>) -> Result<Self, InvalidSvg> {
        let bytes = bytes.as_ref();
        if bytes.len() > MAX_SVG_BYTES {
            return Err(InvalidSvg::limit());
        }
        let src = std::str::from_utf8(bytes).map_err(|_| InvalidSvg::Utf8)?;
        let doc = SvgDocument::parse(src)?;
        let len = doc.serialized_len();
        doc.into_icon(len)
    }

    /// Return the normalized SVG markup without XML comments.
    pub fn svg(&self) -> &str {
        &self.0.svg
    }

    /// Return the validated user-space view box.
    pub fn view_box(&self) -> ViewBox {
        self.0.vb
    }

    /// Return the intrinsic aspect ratio, honoring explicit absolute dimensions if present.
    pub fn aspect_ratio(&self) -> f64 {
        self.0.aspect_ratio
    }

    /// Return the paint classification of reachable rendered content.
    pub fn paint(&self) -> IconPaint {
        self.0.paint
    }

    /// Return an inline SVG copy with local IDs, `href`, `url()`, and ARIA IDs prefixed.
    pub fn svg_with_id_prefix(&self, prefix: &str) -> Result<String, InvalidSvg> {
        crate::identity::validate_name(prefix).map_err(InvalidSvg::InvalidIdPrefix)?;
        if self.0.longest_id == 0 {
            return Ok(self.svg().to_owned());
        }
        // Length-delimit the prefix so distinct `(prefix, id)` pairs cannot collide.
        let prefix = format!("{}-{prefix}-", prefix.len());
        let byte_len = prefix.len() + self.0.longest_id;
        if byte_len > MAX_SVG_ID_BYTES {
            return Err(InvalidSvg::PrefixedIdTooLong { byte_len });
        }
        let mut size = InlineSize(0);
        self.write_inline(&prefix, &mut size)
            .map_err(|_| InvalidSvg::limit())?;
        let mut out = String::with_capacity(size.0);
        self.write_inline(&prefix, &mut out)
            .expect("writing preflighted SVG to String cannot fail");
        Ok(out)
    }

    fn write_inline(&self, prefix: &str, out: &mut impl Write) -> fmt::Result {
        let mut pos = 0;
        for span in &self.0.refs {
            out.write_str(&self.svg()[pos..span.range.start])?;
            span.write_prefixed(prefix, out)?;
            pos = span.range.end;
        }
        out.write_str(&self.svg()[pos..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(body: &str) -> Result<SvgIcon, InvalidSvg> {
        SvgIcon::parse(format!(r#"<svg viewBox="0 0 24 24">{body}</svg>"#))
    }

    type Refusal = fn(&InvalidSvg) -> bool;

    #[test]
    fn invalid_icons_rejected() {
        let cases: [(&str, Refusal); 14] = [
            ("<path", |e| matches!(e, InvalidSvg::Xml { .. })),
            (r#"<path d="M 0"/>"#, |e| matches!(e, InvalidSvg::Path)),
            (r#"<path d="M0 0 L"/>"#, |e| matches!(e, InvalidSvg::Path)),
            (r#"<path d="R0 0"/>"#, |e| matches!(e, InvalidSvg::Path)),
            (r#"<path d="M0 0A1 1 0 3 0 4 4"/>"#, |e| {
                matches!(e, InvalidSvg::Path)
            }),
            (r#"<path d="M1e999 0"/>"#, |e| matches!(e, InvalidSvg::Path)),
            (r##"<path fill="url(#missing)"/>"##, |e| {
                matches!(e, InvalidSvg::MissingReference { .. })
            }),
            (r##"<path id="shape"/><path fill="url(#shape)"/>"##, |e| {
                matches!(e, InvalidSvg::ReferenceTarget { .. })
            }),
            (
                r##"<defs><g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g></defs>"##,
                |e| matches!(e, InvalidSvg::ReferenceCycle),
            ),
            (r##"<g id="a"><use href="#a"/></g>"##, |e| {
                matches!(e, InvalidSvg::ReferenceCycle)
            }),
            (r##"<g id="a"/><g id="a"/>"##, |e| {
                matches!(e, InvalidSvg::DuplicateId { .. })
            }),
            (r#"<use href="icons.svg#mark"/>"#, |e| {
                matches!(e, InvalidSvg::ExternalReference)
            }),
            ("<script/>", |e| matches!(e, InvalidSvg::Element { .. })),
            (r#"<path xmlns=""/>"#, |e| {
                matches!(e, InvalidSvg::Element { .. })
            }),
        ];
        for (body, pred) in cases {
            let err = icon(body).expect_err(body);
            assert!(pred(&err), "{body}: {err:?}");
        }
    }

    #[test]
    fn normalization_canonicalizes_root() {
        let src = r#"<svg fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" d="M2 2L22 22"/></svg>"#;
        let icon = SvgIcon::parse(src).unwrap();
        assert_eq!(icon.paint(), IconPaint::CurrentColor);
        assert!(icon.svg().contains(r#"fill="none""#));
        assert!(icon.svg().contains(r#"stroke-width="2""#));
        assert_eq!(SvgIcon::parse(icon.svg()).unwrap().svg(), icon.svg());
        let reordered = src.replace(
            r#"fill="none" stroke="currentColor""#,
            r#"stroke="currentColor" fill="none""#,
        );
        assert_eq!(SvgIcon::parse(reordered).unwrap().svg(), icon.svg());
    }

    #[test]
    fn prefix_rewrites_all_refs() {
        let icon = icon(r##"<title id="title">Mark</title><defs><linearGradient id="paint"><stop stop-color="red"/></linearGradient><path id="shape"/></defs><use href="#shape" fill="url('#paint')" aria-labelledby="title" style="stroke: url(#paint); opacity: .5"/>"##).unwrap();
        let svg = icon.svg_with_id_prefix("instance-1").unwrap();
        assert!(svg.contains(r#"id="10-instance-1-title""#));
        assert!(svg.contains(r##"href="#10-instance-1-shape""##));
        assert!(svg.contains("#10-instance-1-paint"));
        assert!(svg.contains(r#"aria-labelledby="10-instance-1-title""#));
        let copied = SvgIcon::parse(&svg).unwrap();
        assert_eq!(copied.paint(), icon.paint());
        assert!(copied.svg().contains("opacity: .5"));
        assert_ne!(svg, icon.svg_with_id_prefix("instance-2").unwrap());
    }

    #[test]
    fn prefix_id_pairs_disjoint() {
        let a = icon(r##"<defs><path id="b-c" d="M0 0h12v12z" fill="currentColor"/></defs><use href="#b-c"/>"##)
            .unwrap()
            .svg_with_id_prefix("a")
            .unwrap();
        let b = icon(r##"<defs><path id="c" d="M0 0h12v12z" fill="red"/></defs><use href="#c"/>"##)
            .unwrap()
            .svg_with_id_prefix("a-b")
            .unwrap();
        assert_eq!(icon(&format!("{a}{b}")).unwrap().paint(), IconPaint::Mixed);
    }

    #[test]
    fn aria_prefix_size_limit() {
        let icon = icon(&format!(
            r#"<g id="a"/><path aria-labelledby="{}"/>"#,
            "a ".repeat(9_000)
        ))
        .unwrap();
        assert!(matches!(
            icon.svg_with_id_prefix(&"p".repeat(500)),
            Err(InvalidSvg::Limit { .. })
        ));
        let short = icon.svg_with_id_prefix("p").unwrap();
        assert_eq!(SvgIcon::parse(short).unwrap().paint(), icon.paint());
    }
}
