//! The status line and the key hints under a surface's content.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::terminal::style::Palette;

use super::Action;
use super::keymap;

/// Draws `caption` above the hints `table` spells for `actions`; an empty caption is left out.
pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    caption: &str,
    actions: &[Action],
    table: &keymap::Table,
    palette: Palette,
) {
    let mut lines = Vec::new();
    if !caption.is_empty() {
        lines.push(Line::from(Span::styled(caption, palette.notice_style())));
    }
    let mut hints = Vec::new();
    for (label, spellings) in grouped(table, actions) {
        if !hints.is_empty() {
            hints.push(Span::raw("  "));
        }
        for (position, spelling) in spellings.iter().enumerate() {
            if position > 0 {
                hints.push(Span::styled("/", palette.dim_style()));
            }
            hints.push(Span::styled(*spelling, palette.accent_style()));
        }
        if !label.is_empty() {
            hints.push(Span::raw(" "));
            hints.push(Span::styled(label, palette.dim_style()));
        }
    }
    lines.push(Line::from(clipped(hints, usize::from(area.width))));
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// The keys one action's row shows, in table order: one group per label, each spelling once, so
/// the reader sees `↓/↑ move`, not a list of every key.
fn grouped(table: &keymap::Table, actions: &[Action]) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for (keys, label) in table.hints(actions) {
        let spelled = keys
            .iter()
            .filter(|key| key.spelled_in_hints())
            .map(|key| key.spelling())
            .collect::<Vec<_>>();
        // A group with nothing but arrows keeps the one a reader looks for.
        let spelled = if spelled.is_empty() {
            keys.first().map(|key| key.spelling()).into_iter().collect()
        } else {
            spelled
        };
        match groups.iter_mut().find(|(group, _)| *group == label) {
            Some((_, spellings)) => {
                for spelling in spelled {
                    if !spellings.contains(&spelling) {
                        spellings.push(spelling);
                    }
                }
            }
            None => groups.push((label, spelled)),
        }
    }
    groups
}

/// The spans trimmed to `width` columns, ending with an ellipsis when anything was cut.
///
/// Span-aware rather than a call to the elide helper: each span holds the style the key table
/// gives it, which a plain string would drop.
///
/// Trimming happens on grapheme boundaries: half a grapheme is not text, and a wide grapheme
/// that would straddle the edge goes whole. A row that fits is returned untouched, so nothing
/// is spelled differently than the table spells it.
fn clipped(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    if spans.iter().map(|span| span.content.width()).sum::<usize>() <= width {
        return spans;
    }
    let limit = width.saturating_sub(1);
    let mut used = 0;
    let mut kept: Vec<Span<'static>> = Vec::new();
    'spans: for span in spans {
        let mut text = String::new();
        for grapheme in span.content.graphemes(true) {
            let columns = grapheme.width();
            if used + columns > limit {
                if !text.is_empty() {
                    kept.push(Span::styled(text, span.style));
                }
                break 'spans;
            }
            used += columns;
            text.push_str(grapheme);
        }
        if !text.is_empty() {
            kept.push(Span::styled(text, span.style));
        }
    }
    kept.push(Span::raw("…"));
    kept
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    /// The text one frame drew, line by line.
    fn drawn(status: &str, actions: &[Action], area: Rect) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    area,
                    status,
                    actions,
                    &keymap::DEFAULT,
                    Palette::new(false),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(usize::from(area.width))
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// The text of one clipped row, read back for its display width.
    fn row_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn row_wider_than_the_frame_ends_with_ellipsis() {
        let lines = drawn("working", &[Action::Quit], Rect::new(0, 0, 5, 2));

        assert_eq!(lines[1], "q qu…", "a cut row says so");
    }

    #[test]
    fn fitting_row_is_left_alone() {
        let untouched = clipped(vec![Span::raw("q quit")], 20);
        let too_narrow = clipped(vec![Span::raw("q quit")], 6);

        assert_eq!(row_text(&untouched), "q quit", "no ellipsis is added");
        assert_eq!(
            row_text(&too_narrow),
            "q quit",
            "a row exactly as wide fits"
        );
        assert_eq!(untouched.len(), 1, "the span the table built is kept");
    }

    #[test]
    fn clipping_ends_on_grapheme_boundaries() {
        // Three wide graphemes need six columns; the fourth would straddle the fifth.
        let spans = clipped(vec![Span::raw("名称裁剪测试")], 5);

        assert_eq!(row_text(&spans), "名称…");
        assert!(row_text(&spans).width() <= 5, "{:?}", row_text(&spans));
    }

    #[test]
    fn hints_name_only_accepted_actions() {
        let lines = drawn(
            "3 of 40 rows",
            &[Action::Quit, Action::Open, Action::Export],
            Rect::new(0, 0, 48, 2),
        );
        assert_eq!(lines[0], "3 of 40 rows");
        assert!(lines[1].contains("Enter open"), "{}", lines[1]);
        assert!(lines[1].contains("e export"), "{}", lines[1]);
        assert!(lines[1].contains("q quit"), "{}", lines[1]);
        assert!(!lines[1].contains("search"), "{}", lines[1]);
    }

    #[test]
    fn empty_status_draws_only_hints() {
        let lines = drawn("", &[Action::Quit], Rect::new(0, 0, 24, 1));
        assert_eq!(lines[0], "q quit");
    }
}
