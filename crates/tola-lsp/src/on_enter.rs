//! The edits Enter applies in one source, as the protocol has them.

use lsp_types::{Position, TextEdit};
use tola_typst_syntax::typst_syntax::Source;

use crate::position;

/// The edits a client applies in place of Enter's own behaviour at `position`.
///
/// A source that continues nothing answers nothing, so the editor inserts its own newline.
pub(super) fn edits(source: &Source, position: Position) -> Option<Vec<TextEdit>> {
    let byte = position::byte_offset(source.lines(), position).ok()?;
    // `continuations` addresses the compiler's lines; the edits it answers are byte ranges.
    let at = tola_typst_syntax::position::utf16_range(source.lines(), byte..byte)?.start;
    tola_typst_syntax::continuation::continuations(source, at)?
        .into_iter()
        .map(|edit| {
            let range = position::utf16_range(source.lines(), edit.range)?;
            // The caret the answer leaves becomes the tab stop the client places it at.
            let mut text = edit.text;
            if let Some(insertion) = edit.insertion {
                text.insert_str(insertion, "$0");
            }
            Some(TextEdit {
                range,
                new_text: text,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_follows_protocol_cursor() {
        for separator in ['\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'] {
            let source = Source::detached(format!("Body{separator}/// note"));
            let at = Position::new(0, position::utf16_len(source.text()) as u32);
            let edits = edits(&source, at).expect("the documentation comment continues");
            assert_eq!(edits[0].range, lsp_types::Range::new(at, at));
            assert_eq!(edits[0].new_text, "\n/// $0");
        }
    }
}
