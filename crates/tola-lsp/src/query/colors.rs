//! The colours one source writes, as the protocol has them.

use anyhow::{Result, anyhow};
use lsp_types::{Color, ColorInformation, ColorPresentation, Range, TextEdit};
use tola_typst_syntax::colours;
use tola_typst_syntax::typst_library::visualize::Color as TypstColor;
use tola_typst_syntax::typst_syntax::Source;

use crate::position;

/// Every colour the source writes, in document order.
pub(super) fn document(source: &Source) -> Result<Vec<ColorInformation>> {
    let mut colours = Vec::new();
    for colour in colours::colours(source) {
        let range = position::utf16_range(source.lines(), colour.range)
            .ok_or_else(|| anyhow!("colour range exceeds the document"))?;
        colours.push(ColorInformation {
            range,
            color: carried(colour.value),
        });
    }
    Ok(colours)
}

/// The forms the author can paste back for one colour.
///
/// `rgb` accepts both spellings: the hexadecimal notation, which has alpha as its fourth pair
/// of digits, and whole components out of 255 with a ratio for alpha.
pub(super) fn presentations(color: Color, range: Range) -> Vec<ColorPresentation> {
    colours::spellings([color.red, color.green, color.blue, color.alpha])
        .into_iter()
        .map(|new_text| ColorPresentation {
            label: new_text.clone(),
            text_edit: Some(TextEdit { range, new_text }),
            additional_text_edits: None,
        })
        .collect()
}

/// The protocol's colour for a Typst colour.
fn carried(colour: TypstColor) -> Color {
    let (red, green, blue, alpha) = colour.to_rgb().into_components();
    Color {
        red,
        green,
        blue,
        alpha,
    }
}
