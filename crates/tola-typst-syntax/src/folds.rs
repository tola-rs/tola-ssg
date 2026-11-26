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
pub fn folds(src: &Source) -> Option<Vec<Fold>> {
    let lines = src.lines();
    let mut ranges = Vec::new();
    for section in sections::sections(src) {
        push_range(
            &mut ranges,
            lines,
            section.heading.start..section.body.end,
            None,
            Some(section.name),
        );
    }
    fold_regions(&LinkedNode::new(src.root()), lines, &mut ranges);
    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    // Call args and content block can share brackets, producing duplicate line pairs.
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
            // Single newline (`Space`) continues a comment run; `Parbreak` ends it.
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
    fn fold_section_body_end() {
        let ranges =
            super::folds(&Source::detached("= One\nbody\n\n= Two\nmore\n")).expect("section folds");
        assert_eq!((ranges[0].end_line, ranges[0].end_character), (1, Some(4)));
    }

    #[test]
    fn fold_trims_trailing_blank_lines() {
        assert_eq!(
            folded("= One\nfirst\n\n\n"),
            [(0, 1, Some("One".to_owned()))]
        );
        assert_eq!(folded("= One\n"), Vec::new());
    }

    #[test]
    fn fold_list_items() {
        assert_eq!(folded("- first\n  continued\n- second\n"), [(0, 1, None)]);
    }
}
