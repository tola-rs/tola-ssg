//! Formatting one Typst source.

use std::ops::Range;

use typst_syntax::{LinkedNode, Source, ast};
use typstyle_core::pretty::PrettyPrinter;
use typstyle_core::{AttrStore, Config, Typstyle, WrapMode};

use crate::edit::Edit;
use crate::style::{Options, ProseWrap};

const MINIMUM_PRINT_WIDTH: usize = 20;

/// What formatting one source produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Formatting {
    /// The source already stands as it should.
    Stands,
    /// The edit that reformats the source.
    Reformatted(Edit),
    /// The source has syntax errors; the author's text stands.
    Unparsable {
        /// The byte offset the tree marks as the first error, when it marks a place.
        offset: Option<usize>,
    },
    /// The printer could not produce a replacement that preserves protected text and values.
    Unrenderable,
}

/// What formatting `source` produced. A rejected edit leaves the author's source in place.
pub fn format(source: &str, options: &Options) -> Formatting {
    let src = Source::detached(source);
    if src.root().diagnosis().errors {
        return Formatting::Unparsable {
            offset: crate::errors::first_error(&src).map(|range| range.start),
        };
    }

    // Typstyle's whole-source renderer trims every line, including literal text.
    let printer = PrettyPrinter::new(config(options), AttrStore::new(src.root()));
    let markup = src.root().cast().expect("source root is markup");
    let doc = printer.convert_markup(Default::default(), markup);
    let mut printed = String::new();
    if doc
        .render_fmt(printer.config().max_width, &mut printed)
        .is_err()
    {
        return Formatting::Unrenderable;
    }

    match checked_edit(&src, 0..src.text().len(), &printed, true) {
        Ok(Some(edit)) => Formatting::Reformatted(edit),
        Ok(None) => Formatting::Stands,
        Err(()) => Formatting::Unrenderable,
    }
}

/// The edit that formats the constructs covered by the byte range.
pub fn format_range(source: &str, range: Range<usize>, options: &Options) -> Option<Edit> {
    if source.get(range.clone())?.trim().is_empty() {
        return None;
    }
    let src = Source::detached(source);
    if src.root().diagnosis().errors {
        return None;
    }

    let reprinted = Typstyle::new(config(options))
        .format_source_range(src.clone(), range)
        .ok()?;
    checked_edit(&src, reprinted.source_range, &reprinted.content, false)
        .ok()
        .flatten()
}

fn checked_edit(
    src: &Source,
    range: Range<usize>,
    printed: &str,
    whole: bool,
) -> Result<Option<Edit>, ()> {
    let text = src.text();
    let previous = text.get(range.clone()).ok_or(())?;
    if !whole && printed == previous {
        return Ok(None);
    }
    let candidate = replaced(text, range.clone(), printed);
    if candidate.root().diagnosis().errors {
        return Err(());
    }
    let before = protected(src);
    let after = protected(&candidate);
    if before.len() != after.len() {
        return Err(());
    }

    let newline = line_ending(text);
    let mut replacement = String::new();
    let mut cursor = range.start;
    let end = range.start + printed.len();
    for ((old, disabled), (new, now_disabled)) in before.iter().zip(&after) {
        let span = new.range();
        if span.end <= cursor || span.start >= end {
            continue;
        }

        let start = span.start.max(cursor);
        let stop = span.end.min(end);
        append_lines(
            &mut replacement,
            &candidate.text()[cursor..start],
            newline,
            whole,
        );

        let new_text = &candidate.text()[start..stop];
        let old_text = &text[old.range()];
        if disabled == now_disabled
            && old.kind() == new.kind()
            && (*disabled || old.is::<ast::Raw>())
            && start == span.start
            && stop == span.end
            && old_text.lines().eq(new_text.lines())
            && old_text.ends_with('\n') == new_text.ends_with('\n')
        {
            // Preserve original CRLF inside protected nodes when line content is unchanged.
            replacement.push_str(old_text);
        } else {
            replacement.push_str(new_text);
        }
        cursor = stop;
    }
    let tail = &candidate.text()[cursor..end];
    if whole && !printed.ends_with('\n') {
        append_lines(
            &mut replacement,
            tail.trim_end_matches(|c: char| c.is_whitespace() && c != '\n'),
            newline,
            whole,
        );
        replacement.push_str(newline);
    } else {
        append_lines(&mut replacement, tail, newline, whole);
    }
    if replacement == previous {
        return Ok(None);
    }

    let candidate = replaced(text, range.clone(), &replacement);
    let after = protected(&candidate);
    if candidate.root().diagnosis().errors
        || before.len() != after.len()
        || !before.iter().zip(&after).all(|((old, d0), (new, d1))| {
            protected_match(old, *d0, new, *d1, text, candidate.text())
        })
    {
        return Err(());
    }

    Ok(Some(Edit {
        range,
        text: replacement,
        insertion: None,
    }))
}

fn line_ending(text: &str) -> &'static str {
    text.find(['\r', '\n'])
        .map(|at| match &text.as_bytes()[at..] {
            [b'\r', b'\n', ..] => "\r\n",
            [b'\r', ..] => "\r",
            _ => "\n",
        })
        .unwrap_or("\n")
}

fn protected_match(
    old: &LinkedNode<'_>,
    old_disabled: bool,
    new: &LinkedNode<'_>,
    new_disabled: bool,
    old_text: &str,
    new_text: &str,
) -> bool {
    if old_disabled != new_disabled || old.kind() != new.kind() {
        return false;
    }
    if old_disabled {
        return old_text[old.range()] == new_text[new.range()];
    }
    if let (Some(old), Some(new)) = (old.cast::<ast::Str>(), new.cast::<ast::Str>()) {
        return old.get() == new.get();
    }
    if let (Some(old), Some(new)) = (old.cast::<ast::Raw>(), new.cast::<ast::Raw>()) {
        return old.block() == new.block()
            && old.lang().map(ast::RawLang::get) == new.lang().map(ast::RawLang::get)
            && old
                .lines()
                .map(ast::Text::get)
                .eq(new.lines().map(ast::Text::get));
    }
    false
}

fn replaced(src: &str, range: Range<usize>, text: &str) -> Source {
    let mut candidate = src.to_owned();
    candidate.replace_range(range, text);
    Source::detached(candidate)
}

fn protected(src: &Source) -> Vec<(LinkedNode<'_>, bool)> {
    fn collect<'a>(
        node: LinkedNode<'a>,
        attrs: &AttrStore,
        spans: &mut Vec<(LinkedNode<'a>, bool)>,
    ) {
        let disabled = attrs.is_format_disabled(node.get());
        if disabled || node.is::<ast::Str>() || node.is::<ast::Raw>() {
            spans.push((node, disabled));
        } else {
            for child in node.children() {
                collect(child, attrs, spans);
            }
        }
    }

    let attrs = AttrStore::new(src.root());
    let mut spans = Vec::new();
    collect(LinkedNode::new(src.root()), &attrs, &mut spans);
    spans
}

fn append_lines(out: &mut String, text: &str, newline: &str, trim: bool) {
    for part in text.split_inclusive('\n') {
        if let Some(line) = part.strip_suffix('\n') {
            let line = if trim {
                line.trim_end()
            } else {
                line.strip_suffix('\r').unwrap_or(line)
            };
            out.push_str(line);
            out.push_str(newline);
        } else {
            out.push_str(part);
        }
    }
}

fn config(options: &Options) -> Config {
    Config {
        tab_spaces: options.tab_spaces,
        max_width: options.max_width.max(MINIMUM_PRINT_WIDTH),
        wrap_mode: match options.prose_wrap {
            ProseWrap::None => WrapMode::None,
            ProseWrap::Fill => WrapMode::Fill,
            ProseWrap::Sentence => WrapMode::Sentence,
        },
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_range_construct() {
        let src = "= Head\n\n#let  x =  1\n";
        let start = src.find("#let").expect("the binding");
        let edit = format_range(src, start..src.len(), &Options::default()).expect("an edit");
        assert_eq!(edit.range, start..src.len() - 1);
        assert_ne!(edit.text, src[edit.range]);
    }

    #[test]
    fn format_preserves_multiline_literals() {
        let src = "#let  x = \"first  \r\nsecond\"\n\n```txt\r\nexample  \r\n```\n";
        let Formatting::Reformatted(edit) = format(src, &Options::default()) else {
            panic!("the declaration needs formatting");
        };
        assert!(edit.text.contains("\"first  \r\nsecond\""));
        assert!(edit.text.contains("```txt\r\nexample  \r\n```"));
    }

    #[test]
    fn format_range_skips_disabled() {
        let src = "// @typstyle off\n#let x=1\n";
        let start = src.find("x=1").unwrap();
        assert!(format_range(src, start..src.len(), &Options::default()).is_none());
    }

    #[test]
    fn format_range_rejects_invalid() {
        let src = "é  \n";
        for range in [0..0, 1..2, 2..src.len(), src.len()..2, 0..src.len() + 1] {
            assert!(format_range(src, range, &Options::default()).is_none());
        }
        assert!(format_range("#let = ", 0..7, &Options::default()).is_none());
    }

    #[test]
    fn format_unparsable_offset() {
        let src = "= Title\n#let = \n";
        let broken = src.find("#let").expect("the broken line");
        let line_end = src[broken..].find('\n').map_or(src.len(), |at| broken + at);
        let offset = match format(src, &Options::default()) {
            Formatting::Unparsable { offset } => offset.expect("the tree marks a place"),
            other => panic!("expected an unparsable source, got {other:?}"),
        };
        assert!((broken..=line_end).contains(&offset), "offset {offset}");
    }
}
