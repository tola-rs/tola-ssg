//! The replacement a formatting request applies to one source.
//!
//! The site's `[formatter]` configuration decides indentation, width, and prose wrapping. The
//! editor's per-request `tabSize` and `insertSpaces` decide nothing here: indentation is the
//! site's.

use lsp_types::TextEdit;
use tola_build::config::section::FormatterConfig;
use tola_typst_syntax::edit::Edit;
use tola_typst_syntax::format::{self, Formatting, Options};
use tola_typst_syntax::typst_syntax::Lines;

use crate::position;

/// The style a formatting answer writes in: the site's `[formatter]` configuration.
///
/// A connection that has not resolved a site's configuration yet formats with the same defaults
/// a site that names no `[formatter]` key gets.
pub(super) fn style(site: Option<&FormatterConfig>) -> Options {
    let defaults = FormatterConfig::default();
    let site = site.unwrap_or(&defaults);
    Options {
        tab_spaces: site.indent,
        max_width: site.print_width,
        prose_wrap: site.prose_wrap,
    }
}

/// The edits that format `source`, or `None` when the editor keeps the text it has.
pub(super) fn edits(source: &str, options: &Options) -> Option<Vec<TextEdit>> {
    match format::format(source, options) {
        Formatting::Reformatted(edit) => carried(source, edit),
        Formatting::Stands | Formatting::Unparsable { .. } | Formatting::Unrenderable => None,
    }
}

/// The edits that format the part of `source` one range covers, or `None` when the editor keeps it.
pub(super) fn range_edits(
    source: &str,
    bytes: std::ops::Range<usize>,
    options: &Options,
) -> Option<Vec<TextEdit>> {
    carried(source, format::format_range(source, bytes, options)?)
}

/// One formatting edit, in the protocol's own positions.
fn carried(source: &str, mut edit: Edit) -> Option<Vec<TextEdit>> {
    if source
        .split_once('\n')
        .is_some_and(|(line, _)| line.ends_with('\r'))
    {
        edit.text = edit.text.replace("\r\n", "\n").replace('\n', "\r\n");
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
    edit.range.start += prefix;
    edit.range.end -= suffix;
    edit.text = edit.text[prefix..edit.text.len() - suffix].to_owned();
    Some(vec![TextEdit {
        range: position::utf16_range(&Lines::new(source), edit.range)?,
        new_text: edit.text,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

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
