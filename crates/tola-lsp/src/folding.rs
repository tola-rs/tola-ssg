//! The folds of one source, as the protocol has them.

use lsp_types::{FoldingRange, FoldingRangeKind, Position};
use tola_typst_syntax::folds::{Fold, FoldKind};
use tola_typst_syntax::position::{self as source, Utf16Position};
use tola_typst_syntax::typst_syntax::{Lines, Source};

use crate::position;

/// The folding ranges of `source`, or `None` when there is nothing to fold.
///
/// A client that folds whole lines only reads lines; one that folds part of a line reads the
/// columns too, so a fold hides exactly what it covers.
pub(super) fn ranges(source: &Source, line_folding_only: bool) -> Option<Vec<FoldingRange>> {
    let folds = tola_typst_syntax::folds::folds(source)?;
    Some(
        folds
            .into_iter()
            .filter_map(|fold| carried(source.lines(), fold, line_folding_only))
            .collect(),
    )
}

/// One fold, told the way the client reads folds.
fn carried(lines: &Lines<String>, fold: Fold, line_folding_only: bool) -> Option<FoldingRange> {
    let start = fold_position(lines, fold.start_line, fold.start_character)?;
    let end = fold_position(lines, fold.end_line, fold.end_character)?;
    let columns = !line_folding_only;
    Some(FoldingRange {
        start_line: start.line,
        start_character: columns.then_some(start.character),
        end_line: end.line,
        end_character: columns.then_some(end.character),
        kind: fold.kind.map(|kind| match kind {
            FoldKind::Comment => FoldingRangeKind::Comment,
        }),
        collapsed_text: fold.label,
    })
}

/// The protocol position one end of a fold addresses.
///
/// The syntax crate's folds hold the compiler's own lines and columns, and the compiler's breaks
/// include five characters the protocol does not break on, so an end is read through its byte.
fn fold_position(lines: &Lines<String>, line: u32, character: Option<u32>) -> Option<Position> {
    let at = source::byte_offset(lines, Utf16Position::new(line, character?)).ok()?;
    position::utf16_range(lines, at..at).map(|at| at.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A break only the compiler knows cannot move the fold's end off the client's line.
    #[test]
    fn fold_end_follows_the_client_line() {
        let source = Source::detached("#let x = {\n  a\u{0B}b\n  c\n}\n");

        let folds = ranges(&source, false).expect("the code block folds");

        let fold = folds.first().expect("one fold");
        assert_eq!(
            (
                fold.start_line,
                fold.start_character,
                fold.end_line,
                fold.end_character,
            ),
            (0, Some(9), 3, Some(1))
        );
    }
}
