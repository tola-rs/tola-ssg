//! Formatting edits derived from the site's `[formatter]` config (not client tab options).

use lsp_types::TextEdit;
use tola_typst_syntax::edit::Edit;
use tola_typst_syntax::format::{self, Formatting};
use tola_typst_syntax::style::Options;
use tola_typst_syntax::typst_syntax::Lines;

use crate::position;

pub(super) fn edits(src: &str, opts: &Options) -> Option<Vec<TextEdit>> {
    match format::format(src, opts) {
        Formatting::Reformatted(edit) => text_edit(src, edit),
        Formatting::Stands | Formatting::Unparsable { .. } | Formatting::Unrenderable => None,
    }
}

pub(super) fn range_edits(
    src: &str,
    bytes: std::ops::Range<usize>,
    opts: &Options,
) -> Option<Vec<TextEdit>> {
    text_edit(src, format::format_range(src, bytes, opts)?)
}

fn text_edit(src: &str, mut edit: Edit) -> Option<Vec<TextEdit>> {
    if splits_crlf(src, edit.range.start) {
        edit.range.start -= 1;
        edit.text.insert(0, '\r');
    }
    if splits_crlf(src, edit.range.end) {
        edit.range.end += 1;
        edit.text.push('\n');
    }
    let prev = &src[edit.range.clone()];
    if edit.text == prev {
        return None;
    }
    let mut prefix = 0;
    for (old, new) in prev.chars().zip(edit.text.chars()) {
        if old != new {
            break;
        }
        prefix += old.len_utf8();
    }
    if splits_crlf(src, edit.range.start + prefix) {
        prefix -= 1;
    }
    let mut suffix = 0;
    for (old, new) in prev[prefix..]
        .chars()
        .rev()
        .zip(edit.text[prefix..].chars().rev())
    {
        if old != new {
            break;
        }
        suffix += old.len_utf8();
    }
    if splits_crlf(src, edit.range.end - suffix) {
        suffix -= 1;
    }
    let bytes = edit.range.start + prefix..edit.range.end - suffix;
    Some(vec![TextEdit {
        range: position::utf16_range(&Lines::new(src), bytes)?,
        new_text: edit.text[prefix..edit.text.len() - suffix].to_owned(),
    }])
}

fn splits_crlf(src: &str, at: usize) -> bool {
    at > 0 && src.as_bytes().get(at - 1..=at) == Some(b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_crlf_intact() {
        let src = "a\nb\r\nc";
        let lines = Lines::new(src);
        for (range, replacement, expected) in [
            (0..src.len(), "a\nb\rc", "a\nb\rc"),
            (0..src.len(), "a\nb\nc", "a\nb\nc"),
            (4..4, "x", "a\nb\rx\nc"),
        ] {
            let edits = text_edit(
                src,
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
            let mut actual = src.to_owned();
            actual.replace_range(bytes, &edit.new_text);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn edits_minimal_span() {
        for nl in ["\n", "\r\n"] {
            let src = format!("= 标题{nl}{nl}#let value=\"🪿\"{nl}");
            let [edit] = edits(&src, &Options::default())
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(edit.range.start, lsp_types::Position::new(2, 10));
            assert_eq!(edit.range.end, lsp_types::Position::new(2, 11));
            assert_eq!(edit.new_text, " = ");
            let lines = Lines::new(&src);
            let start = position::byte_offset(&lines, edit.range.start).unwrap();
            let end = position::byte_offset(&lines, edit.range.end).unwrap();
            let formatted = format!("{}{}{}", &src[..start], edit.new_text, &src[end..]);
            assert_eq!(formatted, format!("= 标题{nl}{nl}#let value = \"🪿\"{nl}"));
        }
    }

    #[test]
    fn formatting_line_endings() {
        let opts = Options {
            max_width: 20,
            ..Options::default()
        };
        for nl in ["\n", "\r\n"] {
            for prefix in ["", "```typ\n#let x = 1\n```\n\n"] {
                let src =
                    format!("{prefix}#let  values = (\n  first:1,\n  second:2,\n  third:3,\n)\n")
                        .replace('\n', nl);
                for changed in [edits(&src, &opts), range_edits(&src, 0..src.len(), &opts)] {
                    let edit = &changed.expect("the declaration needs formatting")[0];
                    let lines = Lines::new(&src);
                    let start = position::byte_offset(&lines, edit.range.start).unwrap();
                    let end = position::byte_offset(&lines, edit.range.end).unwrap();
                    let formatted = format!("{}{}{}", &src[..start], edit.new_text, &src[end..]);
                    assert!(formatted.contains("first: 1"), "{formatted:?}");
                    assert!(!formatted.replace(nl, "").contains('\n'), "{formatted:?}");
                    assert!(edits(&formatted, &opts).is_none(), "{formatted:?}");
                }
            }
        }
    }
}
