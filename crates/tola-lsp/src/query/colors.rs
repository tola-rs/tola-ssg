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
            color: protocol_color(colour.value),
        });
    }
    Ok(colours)
}

/// The forms the author can paste back; an edit is offered only for a range still spelling a
/// colour.
pub(super) fn presentations(source: &Source, color: Color, range: Range) -> Vec<ColorPresentation> {
    let spelled = colours::colours(source)
        .iter()
        .any(|colour| position::utf16_range(source.lines(), colour.range.clone()) == Some(range));
    colours::spellings([color.red, color.green, color.blue, color.alpha])
        .into_iter()
        .map(|new_text| ColorPresentation {
            label: new_text.clone(),
            text_edit: spelled.then_some(TextEdit { range, new_text }),
            additional_text_edits: None,
        })
        .collect()
}

/// The protocol's colour for a Typst colour.
fn protocol_color(colour: TypstColor) -> Color {
    let (red, green, blue, alpha) = colour.to_rgb().into_components();
    Color {
        red,
        green,
        blue,
        alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    const RED: &str = "#let brand = rgb(\"#ff0000\")\n";

    /// Whether each presentation of one echoed range carries an edit.
    fn edits_offered(range: Range) -> Vec<bool> {
        let source = Source::detached(RED.to_owned());
        let color = Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        };
        presentations(&source, color, range)
            .into_iter()
            .map(|presentation| presentation.text_edit.is_some())
            .collect()
    }

    #[test]
    fn written_range_offers_edits() {
        let source = Source::detached(RED.to_owned());
        let spelled =
            position::utf16_range(source.lines(), colours::colours(&source)[0].range.clone())
                .expect("the colour's own range");
        assert!(edits_offered(spelled).iter().all(|edit| *edit));
    }

    #[test]
    fn moved_range_offers_labels_only() {
        let moved = Range::new(Position::new(0, 0), Position::new(0, 8));
        assert!(edits_offered(moved).iter().all(|edit| !*edit));
    }
}
