//! Formatting one Typst source.

use std::ops::Range;

use crate::edit::Edit;
use crate::style::{Options, ProseWrap};
use typst_syntax::{LinkedNode, Source, ast};
use typstyle_core::pretty::PrettyPrinter;
use typstyle_core::{AttrStore, Config, Typstyle, WrapMode};

/// The narrowest width a source may be wrapped to.
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
    let source = Source::detached(source);
    if source.root().diagnosis().errors {
        return Formatting::Unparsable {
            offset: crate::errors::first_error(&source).map(|range| range.start),
        };
    }

    // Typstyle's whole-source renderer trims every line, including literal text.
    let printer = PrettyPrinter::new(config(options), AttrStore::new(source.root()));
    let markup = source.root().cast().expect("source root is markup");
    let doc = printer.convert_markup(Default::default(), markup);
    let mut printed = String::new();
    if doc
        .render_fmt(printer.config().max_width, &mut printed)
        .is_err()
    {
        return Formatting::Unrenderable;
    }

    match checked_edit(&source, 0..source.text().len(), &printed, true) {
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
    let source = Source::detached(source);
    if source.root().diagnosis().errors {
        return None;
    }

    let reprinted = Typstyle::new(config(options))
        .format_source_range(source.clone(), range)
        .ok()?;
    checked_edit(&source, reprinted.source_range, &reprinted.content, false)
        .ok()
        .flatten()
}

fn checked_edit(
    source: &Source,
    range: Range<usize>,
    printed: &str,
    whole: bool,
) -> Result<Option<Edit>, ()> {
    let text = source.text();
    let previous = text.get(range.clone()).ok_or(())?;
    if !whole && printed == previous {
        return Ok(None);
    }
    let candidate = replaced(text, range.clone(), printed);
    if candidate.root().diagnosis().errors {
        return Err(());
    }
    let before = protected(source);
    let after = protected(&candidate);
    if before.len() != after.len() {
        return Err(());
    }

    // The first physical line ending sets the convention outside protected spans.
    let newline = text
        .find(['\r', '\n'])
        .map(|at| match &text.as_bytes()[at..] {
            [b'\r', b'\n', ..] => "\r\n",
            [b'\r', ..] => "\r",
            _ => "\n",
        })
        .unwrap_or("\n");
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
            // Keep the original spelling when the printer only changed CRLF inside the node.
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

    // Parse the complete source with the actual replacement, in its original context.
    let candidate = replaced(text, range.clone(), &replacement);
    let after = protected(&candidate);
    if candidate.root().diagnosis().errors
        || before.len() != after.len()
        || !before
            .iter()
            .zip(&after)
            .all(|((old, disabled), (new, now_disabled))| {
                disabled == now_disabled
                    && old.kind() == new.kind()
                    && if *disabled {
                        text[old.range()] == candidate.text()[new.range()]
                    } else if let (Some(old), Some(new)) =
                        (old.cast::<ast::Str>(), new.cast::<ast::Str>())
                    {
                        old.get() == new.get()
                    } else if let (Some(old), Some(new)) =
                        (old.cast::<ast::Raw>(), new.cast::<ast::Raw>())
                    {
                        old.block() == new.block()
                            && old.lang().map(ast::RawLang::get)
                                == new.lang().map(ast::RawLang::get)
                            && old
                                .lines()
                                .map(ast::Text::get)
                                .eq(new.lines().map(ast::Text::get))
                    } else {
                        false
                    }
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

fn replaced(source: &str, range: Range<usize>, text: &str) -> Source {
    let mut candidate = source.to_owned();
    candidate.replace_range(range, text);
    Source::detached(candidate)
}

/// The outermost disabled nodes and literal nodes, in source order.
fn protected(source: &Source) -> Vec<(LinkedNode<'_>, bool)> {
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

    let attrs = AttrStore::new(source.root());
    let mut spans = Vec::new();
    collect(LinkedNode::new(source.root()), &attrs, &mut spans);
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

/// The style a caller's options ask for, in the formatter's own terms.
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
    fn range_answers_the_construct_it_covers() {
        let source = "= Head\n\n#let  x =  1\n";
        let start = source.find("#let").expect("the binding");
        let edit = format_range(source, start..source.len(), &Options::default()).expect("an edit");
        assert_eq!(edit.range, start..source.len() - 1);
        assert_ne!(edit.text, source[edit.range]);
    }

    #[test]
    fn formatting_preserves_multiline_string_and_raw_text() {
        let source = "#let  x = \"first  \r\nsecond\"\n\n```txt\r\nexample  \r\n```\n";
        let Formatting::Reformatted(edit) = format(source, &Options::default()) else {
            panic!("the declaration needs formatting");
        };
        assert!(edit.text.contains("\"first  \r\nsecond\""));
        assert!(edit.text.contains("```txt\r\nexample  \r\n```"));
    }

    #[test]
    fn range_cannot_edit_a_node_disabled_by_an_earlier_comment() {
        let source = "// @typstyle off\n#let x=1\n";
        let start = source.find("x=1").unwrap();
        assert!(format_range(source, start..source.len(), &Options::default()).is_none());
    }

    #[test]
    fn invalid_ranges_and_syntax_yield_no_edit() {
        let source = "é  \n";
        for range in [
            0..0,
            1..2,
            2..source.len(),
            source.len()..2,
            0..source.len() + 1,
        ] {
            assert!(format_range(source, range, &Options::default()).is_none());
        }
        assert!(format_range("#let = ", 0..7, &Options::default()).is_none());
    }

    #[test]
    fn unparsable_source_names_where_it_fails() {
        let source = "= Title\n#let = \n";
        let broken = source.find("#let").expect("the broken line");
        let line_end = source[broken..]
            .find('\n')
            .map_or(source.len(), |at| broken + at);
        let offset = match format(source, &Options::default()) {
            Formatting::Unparsable { offset } => offset.expect("the tree marks a place"),
            other => panic!("expected an unparsable source, got {other:?}"),
        };
        assert!((broken..=line_end).contains(&offset), "offset {offset}");
    }
}
