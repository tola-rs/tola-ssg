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
    // `nesting` addresses the compiler's lines, so a byte becomes one of its positions here; the
    // byte ranges it answers convert back through the protocol's in `nested`.
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
}
