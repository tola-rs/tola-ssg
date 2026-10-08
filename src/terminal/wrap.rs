//! Breaking drawn text into the lines it fits on.

use std::ops::Range;

use unicode_linebreak::{BreakOpportunity, linebreaks};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The lines `text` draws at `columns` columns, as ranges of its bytes.
///
/// `\n` starts a line, and the piece after a final `\n` starts none. A line begins at its first
/// non-whitespace grapheme and ends without trailing whitespace, filling the columns it can: it
/// ends at the furthest position UAX #14 allows a break at, so prose without spaces — Chinese,
/// for one — still fills its lines, and it backs up to the last break when the fill ends inside
/// a word. A grapheme wider than `columns` stands alone, and text that draws nothing is one
/// empty range.
pub(crate) fn line_ranges(text: &str, columns: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    for paragraph in text.split('\n') {
        if paragraph.is_empty() && offset == text.len() {
            break;
        }
        ranges.extend(
            paragraph_ranges(paragraph, columns)
                .into_iter()
                .map(|range| offset + range.start..offset + range.end),
        );
        offset += paragraph.len() + 1;
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
}

/// The lines one `\n`-free paragraph draws at `columns` columns.
fn paragraph_ranges(paragraph: &str, columns: usize) -> Vec<Range<usize>> {
    let allowed = linebreaks(paragraph)
        .filter_map(|(index, opportunity)| {
            (opportunity == BreakOpportunity::Allowed).then_some(index)
        })
        .collect::<Vec<_>>();
    let mut ranges = Vec::new();
    let mut start = skip_whitespace(paragraph, 0);
    while start < paragraph.len() {
        let mut fill = start;
        let mut used = 0;
        for (relative, grapheme) in paragraph[start..].grapheme_indices(true) {
            if used + grapheme.width() > columns && relative > 0 {
                break;
            }
            fill = start + relative + grapheme.len();
            used += grapheme.width();
        }
        let next = if fill == paragraph.len()
            || paragraph[fill..].starts_with(char::is_whitespace)
            || allowed.binary_search(&fill).is_ok()
        {
            fill
        } else {
            // The fill ends inside a word: back up to the last break, or — the text allowing
            // none — split the word at the fill.
            allowed
                .partition_point(|&position| position < fill)
                .checked_sub(1)
                .and_then(|index| allowed.get(index))
                .copied()
                .filter(|position| *position > start)
                .unwrap_or(fill)
        };
        ranges.push(start..start + paragraph[start..next].trim_end().len());
        start = skip_whitespace(paragraph, next);
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
}

/// The first byte at or after `from` a line draws: leading whitespace stays off it.
fn skip_whitespace(text: &str, from: usize) -> usize {
    text[from..]
        .find(|character: char| !character.is_whitespace())
        .map_or(text.len(), |offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of each line `text` draws at `columns` columns.
    fn drawn(text: &str, columns: usize) -> Vec<String> {
        line_ranges(text, columns)
            .into_iter()
            .map(|range| text[range].to_owned())
            .collect()
    }

    #[test]
    fn words_fill_the_columns() {
        assert_eq!(drawn("one two three", 7), ["one two", "three"]);
        assert_eq!(
            drawn("alpha beta gamma delta", 16),
            ["alpha beta gamma", "delta"]
        );
    }

    #[test]
    fn prose_without_spaces_fills_the_columns() {
        assert_eq!(drawn("ab 中文中文中文", 10), ["ab 中文中", "文中文"]);
    }

    #[test]
    fn a_word_with_no_break_splits_at_the_fill() {
        assert_eq!(drawn("abcdefgh ij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn lines_split_on_newlines() {
        assert_eq!(drawn("a\n\nb\n", 9), ["a", "", "b"]);
        assert_eq!(drawn("", 9), [""]);
    }

    #[test]
    fn a_grapheme_wider_than_the_columns_stands_alone() {
        assert_eq!(drawn("界界", 1), ["界", "界"]);
        assert_eq!(drawn("e\u{301}e\u{301}", 1), ["e\u{301}", "e\u{301}"]);
    }

    #[test]
    fn whitespace_stays_off_the_lines() {
        assert_eq!(drawn("  ab  ", 9), ["ab"]);
    }
}
