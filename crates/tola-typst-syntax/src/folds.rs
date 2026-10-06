//! The folding ranges of one source.

use std::ops::Range;

use typst_syntax::{Lines, LinkedNode, Source, SyntaxKind, is_newline};

use crate::sections;

/// A construct an editor folds away.
pub struct Fold {
    /// The line the construct opens on.
    pub start_line: u32,
    /// The column the construct opens at, when the caller reads columns.
    pub start_character: Option<u32>,
    /// The line the construct closes on.
    pub end_line: u32,
    /// The column the construct closes at, when the caller reads columns.
    pub end_character: Option<u32>,
    /// What the fold holds, when the caller tells kinds apart.
    pub kind: Option<FoldKind>,
    /// The text a caller shows in place of the folded span, when it shows one.
    pub label: Option<String>,
}

/// What a fold holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    /// A run of comment lines.
    Comment,
}

/// The constructs whose own span folds when it covers more than one line.
const FOLDED: &[SyntaxKind] = &[
    SyntaxKind::CodeBlock,
    SyntaxKind::ContentBlock,
    SyntaxKind::Parenthesized,
    SyntaxKind::Destructuring,
    SyntaxKind::Array,
    SyntaxKind::Dict,
    SyntaxKind::Args,
    SyntaxKind::Params,
    SyntaxKind::Raw,
    SyntaxKind::Str,
    SyntaxKind::Equation,
    SyntaxKind::ListItem,
    SyntaxKind::EnumItem,
];

/// Every folding range of the source, or `None` when there is nothing to fold.
///
/// A heading folds its section, a construct of a foldable kind folds its whole span, and comments
/// on consecutive lines fold as one region. Each range spans at least two lines and ends on the
/// last line that carries text, so a folded range never hides only blank lines. Ranges come in
/// document order.
pub fn folds(source: &Source) -> Option<Vec<Fold>> {
    let lines = source.lines();
    let mut ranges = Vec::new();
    for section in sections::sections(source) {
        push_range(
            &mut ranges,
            lines,
            section.heading.start..section.body.end,
            None,
            Some(section.name),
        );
    }
    fold_regions(&LinkedNode::new(source.root()), lines, &mut ranges);
    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    // A call's argument list and the content block it carries open the same brackets, so two
    // constructs can describe one fold.
    ranges.dedup_by_key(|range| (range.start_line, range.end_line));
    (!ranges.is_empty()).then_some(ranges)
}

fn fold_regions(node: &LinkedNode<'_>, lines: &Lines<String>, ranges: &mut Vec<Fold>) {
    let mut comments: Option<Range<usize>> = None;
    for child in node.children() {
        let range = child.range();
        let kind = child.kind();
        match kind {
            SyntaxKind::LineComment => {
                comments = Some(match comments.take() {
                    Some(mut run) => {
                        run.end = range.end;
                        run
                    }
                    None => range,
                });
            }
            // One newline between comments keeps the run; a blank line is a paragraph break.
            SyntaxKind::Space => {}
            _ => {
                if let Some(run) = comments.take() {
                    push_range(ranges, lines, run, Some(FoldKind::Comment), None);
                }
                if kind == SyntaxKind::BlockComment {
                    push_range(ranges, lines, range, Some(FoldKind::Comment), None);
                } else if FOLDED.contains(&kind) {
                    push_range(ranges, lines, range, None, None);
                }
            }
        }
        fold_regions(&child, lines, ranges);
    }
    if let Some(run) = comments {
        push_range(ranges, lines, run, Some(FoldKind::Comment), None);
    }
}

/// Record the range `bytes` spans, when it covers more than one line of text.
/// The UTF-16 column one byte sits at, which a client that folds part of a line asks for.
fn utf16_column(lines: &Lines<String>, byte: usize) -> Option<u32> {
    Some(
        crate::position::utf16_range(lines, byte..byte)?
            .start
            .character,
    )
}

fn push_range(
    ranges: &mut Vec<Fold>,
    lines: &Lines<String>,
    bytes: Range<usize>,
    kind: Option<FoldKind>,
    label: Option<String>,
) {
    let end = bytes.start
        + lines.text()[bytes.start..bytes.end]
            .trim_end_matches(is_newline)
            .len();
    let (Some(start_line), Some(end_line)) =
        (lines.byte_to_line(bytes.start), lines.byte_to_line(end))
    else {
        return;
    };
    let (Ok(start_line), Ok(end_line)) = (u32::try_from(start_line), u32::try_from(end_line))
    else {
        return;
    };
    if end_line > start_line {
        // Both ends are reported, so the fold hides exactly what it covers.
        let (start_character, end_character) =
            (utf16_column(lines, bytes.start), utf16_column(lines, end));
        ranges.push(Fold {
            start_line,
            end_line,
            start_character,
            end_character,
            kind,
            label,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folded(text: &str) -> Vec<(u32, u32, Option<String>)> {
        super::folds(&Source::detached(text))
            .unwrap_or_default()
            .into_iter()
            .map(|range| (range.start_line, range.end_line, range.label))
            .collect()
    }

    #[test]
    fn heading_folds_its_section() {
        assert_eq!(
            folded("= One\nfirst\n\n== Two\nsecond\n"),
            [
                (0, 4, Some("One".to_owned())),
                (3, 4, Some("Two".to_owned())),
            ]
        );
    }

    #[test]
    fn section_fold_reports_trimmed_body_end() {
        let ranges =
            super::folds(&Source::detached("= One\nbody\n\n= Two\nmore\n")).expect("section folds");
        assert_eq!((ranges[0].end_line, ranges[0].end_character), (1, Some(4)));
    }

    /// A trailing blank line is not part of the folded text, and a fold never spans one line.
    #[test]
    fn folds_end_at_the_last_text_line() {
        assert_eq!(
            folded("= One\nfirst\n\n\n"),
            [(0, 1, Some("One".to_owned()))]
        );
        assert_eq!(folded("= One\n"), Vec::new());
    }

    #[test]
    fn brackets_fold_their_body() {
        assert_eq!(folded("#let x = {\n  1\n}\n"), [(0, 2, None)]);
        assert_eq!(
            folded("#let style = (\n  a: 1,\n  b: 2,\n)\n"),
            [(0, 3, None)]
        );
        assert_eq!(folded("#let x = { 1 }\n"), Vec::new());
        assert_eq!(
            folded("#let a = {\n  let b = {\n    1\n  }\n}\n"),
            [(0, 4, None), (1, 3, None)]
        );
    }

    #[test]
    fn body_folds_inside_its_heading_section() {
        assert_eq!(
            folded("= One\n#block[\n  body\n]\n"),
            [(0, 3, Some("One".to_owned())), (1, 3, None)]
        );
    }

    #[test]
    fn list_items_fold_their_own_lines() {
        assert_eq!(folded("- first\n  continued\n- second\n"), [(0, 1, None)]);
    }

    #[test]
    fn equations_fold_their_body() {
        assert_eq!(folded("$\n  a + b\n$\n"), [(0, 2, None)]);
        assert_eq!(
            folded("- Item\n\n  $\n    a + b\n  $\n"),
            [(0, 4, None), (2, 4, None)]
        );
    }

    /// A construct folds its own multi-line span, however its body is delimited.
    #[test]
    fn multiline_constructs_fold_their_span() {
        let cases = [
            ("parenthesized", "#(\n  1\n)\n", 0, 2),
            ("array", "#(\n  1,\n  2,\n)\n", 0, 3),
            ("destructuring", "#let (\n  a,\n  b,\n) = (1, 2)\n", 0, 3),
            ("args", "#f(\n  1,\n  2,\n)\n", 0, 3),
            ("params", "#let sum(\n  a,\n  b,\n) = a + b\n", 0, 3),
            ("enum item", "+ first\n  continued\n", 0, 1),
            ("blocky raw", "```html\n<p>hi</p>\n```\n", 0, 2),
            ("inline raw", "`one\ntwo`\n", 0, 1),
            ("string", "#let page = \"\n  <p>hi</p>\n\"\n", 0, 2),
        ];
        for (construct, text, start, end) in cases {
            assert_eq!(folded(text), [(start, end, None)], "for the {construct}");
        }
    }

    #[test]
    fn comment_runs_fold_with_their_kind() {
        assert_eq!(folded("// One\n// Two\n\nbody\n"), [(0, 1, None)]);
        assert_eq!(folded("// One\n"), Vec::new());
        assert_eq!(folded("// One\n\n// Two\n"), Vec::new());
        let run = super::folds(&Source::detached("// One\n// Two\n")).expect("a comment run");
        assert_eq!(run[0].kind, Some(FoldKind::Comment));
        let block =
            super::folds(&Source::detached("/* One\n   Two */\n")).expect("a block comment");
        assert_eq!(block[0].kind, Some(FoldKind::Comment));
    }
}
