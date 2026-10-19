//! The selection ranges of one source, as the protocol has them.

use lsp_types::{Position, Range, SelectionRange};
use tola_typst_syntax::typst_syntax::{Lines, Source};

use crate::position;

/// Every requested position has one chain; invalid positions reject the entire request.
pub(super) fn ranges(source: &Source, positions: &[Position]) -> Option<Vec<SelectionRange>> {
    if source.text().is_empty() {
        return positions
            .iter()
            .map(|at| {
                position::byte_offset(source.lines(), *at)
                    .ok()
                    .map(|_| SelectionRange {
                        range: Range::default(),
                        parent: None,
                    })
            })
            .collect();
    }
    let at: Vec<_> = positions
        .iter()
        .map(|at| {
            let byte = position::byte_offset(source.lines(), *at).ok()?;
            Some(tola_typst_syntax::position::utf16_range(source.lines(), byte..byte)?.start)
        })
        .collect::<Option<_>>()?;
    tola_typst_syntax::nesting::nesting(source, &at)?
        .into_iter()
        .map(|chain| nested(source.lines(), &chain))
        .collect()
}

fn nested(lines: &Lines<String>, chain: &[std::ops::Range<usize>]) -> Option<SelectionRange> {
    let mut selection: Option<SelectionRange> = None;
    for bytes in chain.iter().rev() {
        selection = Some(SelectionRange {
            range: position::utf16_range(lines, bytes.clone())?,
            parent: selection.map(Box::new),
        });
    }
    selection
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_preserve_position_order() {
        let source = Source::detached("#let title = [🦊]\n#title");
        let positions: Vec<_> = ["🦊", "#title", "title ="]
            .into_iter()
            .map(|marker| {
                let byte = source.text().find(marker).unwrap();
                position::utf16_range(source.lines(), byte..byte)
                    .unwrap()
                    .start
            })
            .collect();
        let ranges = ranges(&source, &positions).unwrap();
        for (selection, at) in ranges.iter().zip(&positions) {
            assert!(selection.range.start <= *at && *at <= selection.range.end);
        }
        assert_eq!(ranges.len(), positions.len());
    }

    #[test]
    fn empty_source_preserves_cursors() {
        let selections = ranges(&Source::detached(""), &[Position::default(); 2]).unwrap();
        assert_eq!(selections.len(), 2);
        assert!(
            selections
                .iter()
                .all(|selection| selection.range == lsp_types::Range::default())
        );
    }

    #[test]
    fn selections_follow_protocol_lines() {
        for separator in ['\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'] {
            let source = Source::detached(format!("#let first = 1{separator}#let second = 2"));
            let byte = source.text().find("second").unwrap() + 3;
            let at = position::utf16_range(source.lines(), byte..byte)
                .unwrap()
                .start;
            let selected = ranges(&source, &[at]).unwrap();
            let selected = &selected[0].range;
            let start = position::byte_offset(source.lines(), selected.start).unwrap();
            let end = position::byte_offset(source.lines(), selected.end).unwrap();
            assert_eq!(&source.text()[start..end], "second", "{separator:?}");
        }
    }

    #[test]
    fn selections_clamp_excessive_columns() {
        for text in ["", "#let value = 1"] {
            let source = Source::detached(text);
            let at_end = Position::new(0, position::utf16_len(text) as u32);
            assert_eq!(
                ranges(&source, &[Position::new(0, 1000)]),
                ranges(&source, &[at_end])
            );
        }
    }
}
