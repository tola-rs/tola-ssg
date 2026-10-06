//! The edits Enter applies in one source, as the protocol has them.

use lsp_types::{Position, TextEdit};
use tola_typst_syntax::typst_syntax::Source;

use crate::position;

/// The edits a client applies in place of Enter's own behaviour at `position`.
///
/// A source that continues nothing answers nothing, so the editor inserts its own newline.
pub(super) fn edits(source: &Source, position: Position) -> Option<Vec<TextEdit>> {
    tola_typst_syntax::continuation::continuations(source, position::utf16(position))?
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
