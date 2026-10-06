//! Normalized SVG serialization and the reference spans retained for inline rendering.

use std::{
    fmt::{self, Write},
    ops::Range,
};

use super::{properties, properties::ReferenceKind, SvgChild, SvgDocument};

const SVG_NAMESPACE_ATTRIBUTE: &str = " xmlns=\"http://www.w3.org/2000/svg\"";

/// The escaped-attribute byte range of one reference-bearing value, retained so inline
/// prefixing need not reparse the serialized SVG.
#[derive(Debug)]
pub(in crate::svg) struct ReferenceSpan {
    pub(in crate::svg) range: Range<usize>,
    kind: ReferenceKind,
    value: String,
}

impl ReferenceSpan {
    pub(in crate::svg) fn write_prefixed(
        &self,
        prefix: &str,
        output: &mut impl Write,
    ) -> fmt::Result {
        properties::prefix_references(self.kind, &self.value, prefix, &mut XmlEscape(output))
    }
}

// Both the size preflight and final output use exactly the same escaping path.
struct XmlEscape<'a, W>(&'a mut W);

impl<W: Write> Write for XmlEscape<'_, W> {
    fn write_str(&mut self, source: &str) -> fmt::Result {
        let mut start = 0;
        for (index, character) in source.char_indices() {
            let Some(escaped) = escaped_xml_char(character) else {
                continue;
            };
            self.0.write_str(&source[start..index])?;
            self.0.write_str(escaped)?;
            start = index + character.len_utf8();
        }
        self.0.write_str(&source[start..])
    }
}

impl SvgDocument {
    /// Exact normalized size, used to reserve collection storage before serialization.
    pub(crate) fn serialized_len(&self) -> usize {
        self.elements
            .iter()
            .enumerate()
            .map(|(index, element)| {
                let opening = 1
                    + element.name.len()
                    + if index == 0 {
                        SVG_NAMESPACE_ATTRIBUTE.len()
                    } else {
                        0
                    };
                let attributes = element
                    .attributes
                    .iter()
                    .map(|(name, value)| 4 + name.len() + escaped_len(value))
                    .sum::<usize>();
                let closing = if element.children.is_empty() {
                    2
                } else {
                    4 + element.name.len()
                };
                let text = element
                    .children
                    .iter()
                    .map(|child| match child {
                        SvgChild::Text(text) => escaped_len(text),
                        SvgChild::Element(_) => 0,
                    })
                    .sum::<usize>();
                opening + attributes + closing + text
            })
            .sum()
    }

    pub(super) fn write(&self, output: &mut String, references: &mut Vec<ReferenceSpan>) {
        self.write_element(0, output, references);
    }

    fn write_element(
        &self,
        index: usize,
        output: &mut String,
        references: &mut Vec<ReferenceSpan>,
    ) {
        let element = &self.elements[index];
        output.push('<');
        output.push_str(&element.name);
        if index == 0 {
            output.push_str(SVG_NAMESPACE_ATTRIBUTE);
        }
        for (attribute, value) in &element.attributes {
            output.push(' ');
            output.push_str(attribute);
            output.push_str("=\"");
            let start = output.len();
            escape_xml(value, output);
            if let Some(kind) =
                properties::reference_attribute(attribute).filter(|kind| kind.has_references(value))
            {
                references.push(ReferenceSpan {
                    range: start..output.len(),
                    kind,
                    value: value.clone(),
                });
            }
            output.push('"');
        }
        if element.children.is_empty() {
            output.push_str("/>");
            return;
        }
        output.push('>');
        for child in &element.children {
            match child {
                SvgChild::Element(child) => self.write_element(*child, output, references),
                SvgChild::Text(text) => escape_xml(text, output),
            }
        }
        let _ = write!(output, "</{}>", element.name);
    }
}

fn escaped_xml_char(character: char) -> Option<&'static str> {
    match character {
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

fn escaped_len(source: &str) -> usize {
    source
        .chars()
        .map(|character| escaped_xml_char(character).map_or(character.len_utf8(), str::len))
        .sum()
}

fn escape_xml(source: &str, output: &mut String) {
    for character in source.chars() {
        if let Some(escaped) = escaped_xml_char(character) {
            output.push_str(escaped);
        } else {
            output.push(character);
        }
    }
}
