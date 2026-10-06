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
                (*at == Position::default()).then_some(SelectionRange {
                    range: Range::default(),
                    parent: None,
                })
            })
            .collect();
    }
    let at: Vec<_> = positions.iter().copied().map(position::utf16).collect();
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
}
