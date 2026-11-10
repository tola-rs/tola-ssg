//! The replacement a formatting request applies to one source.
//!
//! The site's `[formatter]` configuration decides indentation, width, and prose wrapping. The
//! editor's per-request `tabSize` and `insertSpaces` decide nothing here: indentation is the
//! site's.

use lsp_types::TextEdit;
use tola_typst_syntax::edit::Edit;
use tola_typst_syntax::format::{self, Formatting};
use tola_typst_syntax::style::Options;
use tola_typst_syntax::typst_syntax::Lines;

use crate::position;

/// The edits that format `source`, or `None` when the editor keeps the text it has.
pub(super) fn edits(source: &str, options: &Options) -> Option<Vec<TextEdit>> {
    match format::format(source, options) {
        Formatting::Reformatted(edit) => text_edit(source, edit),
        Formatting::Stands | Formatting::Unparsable { .. } | Formatting::Unrenderable => None,
    }
}

/// The edits that format the part of `source` one range covers, or `None` when the editor keeps it.
pub(super) fn range_edits(
    source: &str,
    bytes: std::ops::Range<usize>,
    options: &Options,
) -> Option<Vec<TextEdit>> {
    text_edit(source, format::format_range(source, bytes, options)?)
}

/// One formatting edit, in the protocol's own positions.
fn text_edit(source: &str, mut edit: Edit) -> Option<Vec<TextEdit>> {
    if splits_crlf(source, edit.range.start) {
        edit.range.start -= 1;
        edit.text.insert(0, '\r');
    }
    if splits_crlf(source, edit.range.end) {
        edit.range.end += 1;
        edit.text.push('\n');
    }
    let previous = &source[edit.range.clone()];
    if edit.text == previous {
        return None;
    }
    let mut prefix = 0;
    for (old, new) in previous.chars().zip(edit.text.chars()) {
        if old != new {
            break;
        }
        prefix += old.len_utf8();
    }
    if splits_crlf(source, edit.range.start + prefix) {
        prefix -= 1;
    }
    let mut suffix = 0;
    for (old, new) in previous[prefix..]
        .chars()
        .rev()
        .zip(edit.text[prefix..].chars().rev())
    {
        if old != new {
            break;
        }
        suffix += old.len_utf8();
    }
    if splits_crlf(source, edit.range.end - suffix) {
        suffix -= 1;
    }
    let bytes = edit.range.start + prefix..edit.range.end - suffix;
    Some(vec![TextEdit {
        range: position::utf16_range(&Lines::new(source), bytes)?,
        new_text: edit.text[prefix..edit.text.len() - suffix].to_owned(),
    }])
}

fn splits_crlf(source: &str, at: usize) -> bool {
    at > 0 && source.as_bytes().get(at - 1..=at) == Some(b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_edits_address_whole_crlf_breaks() {
        let source = "a\nb\r\nc";
        let lines = Lines::new(source);
        for (range, replacement, expected) in [
            (0..source.len(), "a\nb\rc", "a\nb\rc"),
            (0..source.len(), "a\nb\nc", "a\nb\nc"),
            (4..4, "x", "a\nb\rx\nc"),
        ] {
            let edits = text_edit(
                source,
                Edit {
                    range,
                    text: replacement.to_owned(),
                    insertion: None,
                },
            )
            .unwrap();
            let edit = &edits[0];
            let bytes = position::byte_range(&lines, edit.range).unwrap();
            assert_eq!(bytes, 3..5);
            let mut actual = source.to_owned();
            actual.replace_range(bytes, &edit.new_text);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn formatting_edits_only_the_changed_span() {
        for newline in ["\n", "\r\n"] {
            let source = format!("= 标题{newline}{newline}#let value=\"🪿\"{newline}");
            let [edit] = edits(&source, &Options::default())
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(edit.range.start, lsp_types::Position::new(2, 10));
            assert_eq!(edit.range.end, lsp_types::Position::new(2, 11));
            assert_eq!(edit.new_text, " = ");
            let lines = Lines::new(&source);
            let start = position::byte_offset(&lines, edit.range.start).unwrap();
            let end = position::byte_offset(&lines, edit.range.end).unwrap();
            let formatted = format!("{}{}{}", &source[..start], edit.new_text, &source[end..]);
            assert_eq!(
                formatted,
                format!("= 标题{newline}{newline}#let value = \"🪿\"{newline}")
            );
        }
    }

    #[test]
    fn formatting_preserves_document_line_endings() {
        let options = Options {
            max_width: 20,
            ..Options::default()
        };
        for newline in ["\n", "\r\n"] {
            for prefix in ["", "```typ\n#let x = 1\n```\n\n"] {
                let source =
                    format!("{prefix}#let  values = (\n  first:1,\n  second:2,\n  third:3,\n)\n")
                        .replace('\n', newline);
                for changed in [
                    edits(&source, &options),
                    range_edits(&source, 0..source.len(), &options),
                ] {
                    let edit = &changed.expect("the declaration needs formatting")[0];
                    let lines = Lines::new(&source);
                    let start = position::byte_offset(&lines, edit.range.start).unwrap();
                    let end = position::byte_offset(&lines, edit.range.end).unwrap();
                    let formatted =
                        format!("{}{}{}", &source[..start], edit.new_text, &source[end..]);
                    assert!(formatted.contains("first: 1"), "{formatted:?}");
                    assert!(
                        !formatted.replace(newline, "").contains('\n'),
                        "{formatted:?}"
                    );
                    assert!(edits(&formatted, &options).is_none(), "{formatted:?}");
                }
            }
        }
    }
}
