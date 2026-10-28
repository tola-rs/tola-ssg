//! Frozen SVG icon values: normalized bytes and the local-reference indexes they retain.

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
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.0 = self
            .0
            .checked_add(value.len())
            .filter(|size| *size <= MAX_SVG_BYTES)
            .ok_or(fmt::Error)?;
        Ok(())
    }
}

/// Paint sources in an icon's rendered content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconPaint {
    /// Visible paints all follow the inherited `currentColor`; suitable for a color-following mask.
    CurrentColor,
    /// Paints are fixed, including multicolor icons and default black. Preserve them as an image.
    Fixed,
    /// Both inherited color and fixed paints contribute, or effects prevent a color-following mask.
    /// Preserve the SVG's colors and effects when rendering.
    Mixed,
}

/// Invalid or unsupported static SVG content.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidSvg {
    /// The source or parsed tree exceeds the bounded icon budget.
    #[error("this SVG is larger than the {limit}-byte icon size limit")]
    Limit {
        /// The byte bound the source, tree or serialized output exceeded.
        limit: usize,
    },
    /// The source is not UTF-8.
    #[error("this SVG is not UTF-8 text")]
    Utf8,
    /// XML parsing failed. Diagnostics omit source content.
    #[error("this SVG is not valid XML at line {line}, column {column}")]
    Xml {
        /// One-based source line.
        line: u32,
        /// One-based source column.
        column: u32,
    },
    /// An SVG root or its namespace is missing or unsupported.
    #[error("this SVG does not have an <svg> root element in the SVG namespace")]
    Root,
    /// Root geometry does not provide a finite positive viewport and view box.
    #[error("this SVG needs a viewBox or positive width and height in absolute units")]
    Geometry,
    /// An element is unsupported in a static, self-contained icon.
    #[error("`{element}` cannot be used in an icon; use a plain shape element instead")]
    Element {
        /// The unsupported element name.
        element: String,
    },
    /// An attribute or inline style property is unsupported.
    #[error("`{attribute}` is not supported on `{element}` in an icon")]
    Attribute {
        /// The containing element.
        element: String,
        /// The unsupported attribute or property.
        attribute: String,
    },
    /// A known SVG attribute contains an invalid value.
    #[error("`{attribute}` on `{element}` has a value an icon cannot use")]
    Value {
        /// The containing element.
        element: String,
        /// The invalid attribute or property.
        attribute: String,
    },
    /// SVG path syntax is invalid.
    #[error("the `d` attribute of this path is not valid SVG path syntax")]
    Path,
    /// A resource reference leaves the icon document.
    #[error("every `href` or `url()` in an icon must point at a `#id` in the same file")]
    ExternalReference,
    /// Two elements have the same identifier.
    #[error("two elements in this SVG use the id `{id}`")]
    DuplicateId {
        /// The duplicated identifier.
        id: String,
    },
    /// A local reference has no target.
    #[error("this SVG references `{id}`, which the file does not define")]
    MissingReference {
        /// The unresolved local identifier.
        id: String,
    },
    /// A local reference points to an element with the wrong SVG role.
    #[error("`{attribute}` cannot reference `{id}`; the target is a different kind of element")]
    ReferenceTarget {
        /// The referencing attribute.
        attribute: String,
        /// The target identifier.
        id: String,
    },
    /// The combined containment and resource-reference graph is cyclic.
    #[error("this SVG's references form a cycle")]
    ReferenceCycle,
    /// The caller's prefix violates the icon-name grammar.
    #[error("the SVG id prefix is invalid: {0}")]
    InvalidIdPrefix(#[source] crate::InvalidIconName),
    /// An ID and its valid prefix cannot fit within the inline identifier limit.
    #[error(
        "an id in this icon would occupy {byte_len} bytes after prefixing; the limit is {MAX_SVG_ID_BYTES}"
    )]
    PrefixedIdTooLong {
        /// The complete length-delimited prefix and identifier length.
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
    view_box: ViewBox,
    aspect_ratio: f64,
    paint: IconPaint,
    longest_id: usize,
    references: Box<[ReferenceSpan]>,
}

/// An immutable, validated and self-contained SVG document.
///
/// Clones share storage. The input buffer can be released after import.
#[derive(Clone, Debug)]
pub struct SvgIcon(Arc<ValidatedSvg>);

impl SvgIcon {
    /// Parse a standalone SVG, rejecting malformed or unsupported content.
    pub fn parse(bytes: impl AsRef<[u8]>) -> Result<Self, InvalidSvg> {
        let bytes = bytes.as_ref();
        if bytes.len() > MAX_SVG_BYTES {
            return Err(InvalidSvg::limit());
        }
        let source = std::str::from_utf8(bytes).map_err(|_| InvalidSvg::Utf8)?;
        let document = SvgDocument::parse(source)?;
        let serialized_len = document.serialized_len();
        document.into_icon(serialized_len)
    }

    /// The complete normalized SVG document, without XML comments.
    pub fn svg(&self) -> &str {
        &self.0.svg
    }

    /// The validated user-space view box.
    pub fn view_box(&self) -> ViewBox {
        self.0.view_box
    }

    /// The intrinsic viewport width divided by height, honoring explicit resolvable dimensions.
    pub fn aspect_ratio(&self) -> f64 {
        self.0.aspect_ratio
    }

    /// The paint classification of reachable rendered content.
    pub fn paint(&self) -> IconPaint {
        self.0.paint
    }

    /// Render an inline copy with local IDs, `href`, presentation `url()`s and ARIA ID
    /// references rewritten to an instance-unique prefix.
    ///
    /// Length-delimited prefix/ID pairs keep distinct pairs from colliding. The complete
    /// escaped output is size-checked before allocation.
    /// A malformed prefix preserves its [`crate::InvalidIconName`] cause; a valid prefix that
    /// makes an identifier too long reports [`InvalidSvg::PrefixedIdTooLong`] instead.
    pub fn svg_with_id_prefix(&self, prefix: &str) -> Result<String, InvalidSvg> {
        crate::identity::validate_name(prefix).map_err(InvalidSvg::InvalidIdPrefix)?;
        if self.0.longest_id == 0 {
            return Ok(self.svg().to_owned());
        }
        let prefix = format!("{}-{prefix}-", prefix.len());
        let byte_len = prefix.len() + self.0.longest_id;
        if byte_len > MAX_SVG_ID_BYTES {
            return Err(InvalidSvg::PrefixedIdTooLong { byte_len });
        }
        let mut size = InlineSize(0);
        self.write_inline(&prefix, &mut size)
            .map_err(|_| InvalidSvg::limit())?;
        let mut svg = String::with_capacity(size.0);
        self.write_inline(&prefix, &mut svg)
            .expect("writing preflighted SVG to a String cannot fail");
        Ok(svg)
    }

    fn write_inline(&self, prefix: &str, output: &mut impl Write) -> fmt::Result {
        let mut copied_until = 0;
        for reference in &self.0.references {
            output.write_str(&self.svg()[copied_until..reference.range.start])?;
            reference.write_prefixed(prefix, output)?;
            copied_until = reference.range.end;
        }
        output.write_str(&self.svg()[copied_until..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(body: &str) -> Result<SvgIcon, InvalidSvg> {
        SvgIcon::parse(format!(r#"<svg viewBox="0 0 24 24">{body}</svg>"#))
    }

    /// What one refused icon must report.
    type Refusal = fn(&InvalidSvg) -> bool;

    /// Every shape an icon refuses, and the variant it names for that shape.
    #[test]
    fn invalid_icons_are_rejected() {
        let cases: [(&str, Refusal); 17] = [
            ("<path", |error| matches!(error, InvalidSvg::Xml { .. })),
            (r#"<path d="M 0"/>"#, |error| {
                matches!(error, InvalidSvg::Path)
            }),
            (r#"<path d="M0 0 L"/>"#, |error| {
                matches!(error, InvalidSvg::Path)
            }),
            (r#"<path d="R0 0"/>"#, |error| {
                matches!(error, InvalidSvg::Path)
            }),
            (r#"<path d="M0 0A1 1 0 3 0 4 4"/>"#, |error| {
                matches!(error, InvalidSvg::Path)
            }),
            (r#"<path d="M1e999 0"/>"#, |error| {
                matches!(error, InvalidSvg::Path)
            }),
            (r##"<path fill="url(#missing)"/>"##, |error| {
                matches!(error, InvalidSvg::MissingReference { .. })
            }),
            (
                r##"<path id="shape"/><path fill="url(#shape)"/>"##,
                |error| matches!(error, InvalidSvg::ReferenceTarget { .. }),
            ),
            (
                r##"<defs><g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g></defs>"##,
                |error| matches!(error, InvalidSvg::ReferenceCycle),
            ),
            (r##"<g id="a"><use href="#a"/></g>"##, |error| {
                matches!(error, InvalidSvg::ReferenceCycle)
            }),
            (
                r##"<defs><linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/></defs>"##,
                |error| matches!(error, InvalidSvg::ReferenceCycle),
            ),
            (r##"<g id="a"/><g id="a"/>"##, |error| {
                matches!(error, InvalidSvg::DuplicateId { .. })
            }),
            (r#"<use href="icons.svg#mark"/>"#, |error| {
                matches!(error, InvalidSvg::ExternalReference)
            }),
            (r#"<use href="https://example.org/mark.svg"/>"#, |error| {
                matches!(error, InvalidSvg::ExternalReference)
            }),
            ("<script/>", |error| {
                matches!(error, InvalidSvg::Element { .. })
            }),
            ("<text>logo</text>", |error| {
                matches!(error, InvalidSvg::Element { .. })
            }),
            (r#"<path xmlns=""/>"#, |error| {
                matches!(error, InvalidSvg::Element { .. })
            }),
        ];
        for (body, expected) in cases {
            let error = icon(body).expect_err(body);
            assert!(expected(&error), "{body}: {error:?}");
        }
    }

    #[test]
    fn normalization_preserves_root_properties() {
        let source = r#"<svg fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" d="M2 2L22 22"/></svg>"#;
        let icon = SvgIcon::parse(source).unwrap();
        assert_eq!(icon.paint(), IconPaint::CurrentColor);
        assert!(icon.svg().contains(r#"fill="none""#));
        assert!(icon.svg().contains(r#"stroke-width="2""#));
        assert_eq!(SvgIcon::parse(icon.svg()).unwrap().svg(), icon.svg());
        let reordered = source.replace(
            r#"fill="none" stroke="currentColor""#,
            r#"stroke="currentColor" fill="none""#,
        );
        assert_eq!(SvgIcon::parse(reordered).unwrap().svg(), icon.svg());
    }

    #[test]
    fn prefixing_rewrites_every_reference() {
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
    fn prefix_id_pairs_do_not_collide() {
        let first =
            icon(r##"<defs><path id="b-c" d="M0 0h12v12z" fill="currentColor"/></defs><use href="#b-c"/>"##)
                .unwrap()
                .svg_with_id_prefix("a")
                .unwrap();
        let second =
            icon(r##"<defs><path id="c" d="M0 0h12v12z" fill="red"/></defs><use href="#c"/>"##)
                .unwrap()
                .svg_with_id_prefix("a-b")
                .unwrap();
        assert_eq!(
            icon(&format!("{first}{second}")).unwrap().paint(),
            IconPaint::Mixed
        );
    }

    #[test]
    fn aria_expansion_respects_the_size_limit() {
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
