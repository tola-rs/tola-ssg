//! Normalized SVG serialization and local-reference byte spans.

use std::{
    fmt::{self, Write},
    ops::Range,
};

use super::{properties, properties::ReferenceKind, SvgChild, SvgDocument};

const SVG_NS_ATTR: &str = " xmlns=\"http://www.w3.org/2000/svg\"";

/// Byte range of a reference-bearing attribute value in the serialized SVG.
#[derive(Debug)]
pub(in crate::svg) struct ReferenceSpan {
    pub(in crate::svg) range: Range<usize>,
    kind: ReferenceKind,
    val: String,
}

impl ReferenceSpan {
    pub(in crate::svg) fn write_prefixed(&self, prefix: &str, out: &mut impl Write) -> fmt::Result {
        properties::prefix_references(self.kind, &self.val, prefix, &mut XmlEscape(out))
    }
}

struct XmlEscape<'a, W>(&'a mut W);

impl<W: Write> Write for XmlEscape<'_, W> {
    fn write_str(&mut self, src: &str) -> fmt::Result {
        let mut start = 0;
        for (idx, ch) in src.char_indices() {
            let Some(esc) = escaped_xml_char(ch) else {
                continue;
            };
            self.0.write_str(&src[start..idx])?;
            self.0.write_str(esc)?;
            start = idx + ch.len_utf8();
        }
        self.0.write_str(&src[start..])
    }
}

impl SvgDocument {
    pub(crate) fn serialized_len(&self) -> usize {
        self.elements
            .iter()
            .enumerate()
            .map(|(idx, el)| {
                let open = 1 + el.name.len() + if idx == 0 { SVG_NS_ATTR.len() } else { 0 };
                let attrs = el
                    .attrs
                    .iter()
                    .map(|(name, val)| 4 + name.len() + escaped_len(val))
                    .sum::<usize>();
                let close = if el.children.is_empty() {
                    2
                } else {
                    4 + el.name.len()
                };
                let text = el
                    .children
                    .iter()
                    .map(|child| match child {
                        SvgChild::Text(t) => escaped_len(t),
                        SvgChild::Element(_) => 0,
                    })
                    .sum::<usize>();
                open + attrs + close + text
            })
            .sum()
    }

    pub(super) fn write(&self, out: &mut String, refs: &mut Vec<ReferenceSpan>) {
        self.write_element(0, out, refs);
    }

    fn write_element(&self, idx: usize, out: &mut String, refs: &mut Vec<ReferenceSpan>) {
        let el = &self.elements[idx];
        out.push('<');
        out.push_str(&el.name);
        if idx == 0 {
            out.push_str(SVG_NS_ATTR);
        }
        for (attr, val) in &el.attrs {
            out.push(' ');
            out.push_str(attr);
            out.push_str("=\"");
            let start = out.len();
            escape_xml(val, out);
            if let Some(kind) =
                properties::reference_attribute(attr).filter(|kind| kind.has_references(val))
            {
                refs.push(ReferenceSpan {
                    range: start..out.len(),
                    kind,
                    val: val.clone(),
                });
            }
            out.push('"');
        }
        if el.children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        for child in &el.children {
            match child {
                SvgChild::Element(c) => self.write_element(*c, out, refs),
                SvgChild::Text(t) => escape_xml(t, out),
            }
        }
        let _ = write!(out, "</{}>", el.name);
    }
}

fn escaped_xml_char(ch: char) -> Option<&'static str> {
    match ch {
        '&' => Some("&amp;"),
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '"' => Some("&quot;"),
        '\r' => Some("&#13;"),
        '\n' => Some("&#10;"),
        '\t' => Some("&#9;"),
        _ => None,
    }
}

fn escaped_len(src: &str) -> usize {
    src.chars()
        .map(|ch| escaped_xml_char(ch).map_or(ch.len_utf8(), str::len))
        .sum()
}

fn escape_xml(src: &str, out: &mut String) {
    for ch in src.chars() {
        if let Some(esc) = escaped_xml_char(ch) {
            out.push_str(esc);
        } else {
            out.push(ch);
        }
    }
}
