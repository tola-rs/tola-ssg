//! The status line and the key hints under a surface's content.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

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
    if area.is_empty() {
        return;
    }
    let mut row = Rect { height: 1, ..area };
    if !caption.is_empty() && area.height > 1 {
        frame.render_widget(
            Paragraph::new(Line::styled(caption, palette.notice_style())),
            row,
        );
        row.y += 1;
    }
    let mut groups = grouped(table, actions);
    let mut line = Line::from(hint_spans(&groups, palette));
    if line.width() > usize::from(area.width) {
        let exits = table
            .hints(&[Action::Quit, Action::Dismiss])
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        groups.sort_by_key(|(label, _)| !exits.contains(label));
        line = Line::from(hint_spans(&groups, palette));
    }
    draw_line(frame, row, &line);
}

fn hint_spans(
    groups: &[(&'static str, Vec<&'static str>)],
    palette: Palette,
) -> Vec<Span<'static>> {
    let mut hints = Vec::new();
    for (label, spellings) in groups {
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
            hints.push(Span::styled(*label, palette.dim_style()));
        }
    }
    hints
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

fn draw_line(frame: &mut Frame, area: Rect, line: &Line<'_>) {
    if area.is_empty() {
        return;
    }
    let truncated = line.width() > usize::from(area.width);
    let end = area.right() - u16::from(truncated);
    let mut column = area.x;
    for span in &line.spans {
        let (next, _) = frame
            .buffer_mut()
            .set_span(column, area.y, span, end - column);
        let written = usize::from(next - column);
        column = next;
        if written < span.width() {
            break;
        }
    }
    if truncated {
        frame
            .buffer_mut()
            .set_string(column, area.y, "…", ratatui::style::Style::default());
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use unicode_width::UnicodeWidthStr;

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

    fn rendered(line: &Line<'_>, area: Rect) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(area.right().max(1), area.bottom().max(1))).unwrap();
        terminal.draw(|frame| draw_line(frame, area, line)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_text(buffer: &Buffer, area: Rect) -> String {
        (area.x..area.right())
            .map(|column| buffer[(column, area.y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn row_wider_than_the_frame_ends_with_ellipsis() {
        let lines = drawn("working", &[Action::Quit], Rect::new(0, 0, 5, 2));

        assert_eq!(lines[1], "q qu…", "a cut row says so");
    }

    #[test]
    fn fitting_row_is_left_alone() {
        for width in [6, 20] {
            let area = Rect::new(3, 2, width, 1);
            let buffer = rendered(&Line::from("q quit"), area);
            assert_eq!(row_text(&buffer, area), "q quit");
        }
    }

    #[test]
    fn clipping_ends_on_grapheme_boundaries() {
        for (width, count) in [(1, 0), (3, 1), (4, 1), (5, 2)] {
            let area = Rect::new(3, 2, width, 1);
            let buffer = rendered(&Line::from("名称裁剪测试"), area);
            for (ordinal, glyph) in ["名", "称"].into_iter().take(count as usize).enumerate() {
                assert_eq!(
                    buffer[(area.x + ordinal as u16 * 2, area.y)].symbol(),
                    glyph
                );
            }
            assert_eq!(buffer[(area.x + count * 2, area.y)].symbol(), "…");
        }
    }

    #[test]
    fn clipping_preserves_styled_prefix() {
        let palette = Palette::new(true);
        let area = Rect::new(3, 2, 3, 1);
        let line = Line::from(vec![
            Span::styled("a名", palette.accent_style()),
            Span::styled("z", palette.dim_style()),
        ]);
        let buffer = rendered(&line, area);
        assert_eq!(row_text(&buffer, area), "a…");
        assert_eq!(
            buffer[(area.x, area.y)].fg,
            palette.accent_style().fg.unwrap()
        );
        assert_eq!(
            buffer[(area.x + 1, area.y)].fg,
            ratatui::style::Color::Reset
        );
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

    #[test]
    fn narrow_footer_keeps_exit_visible() {
        let lines = drawn(
            "3 of 40 rows",
            &[
                Action::Up,
                Action::Down,
                Action::Open,
                Action::Search,
                Action::Quit,
            ],
            Rect::new(0, 0, 18, 1),
        );
        assert!(lines[0].contains("q quit"));
        assert!(lines[0].width() <= 18);
        let area = Rect::new(3, 2, 0, 1);
        assert_eq!(
            rendered(&Line::from("text"), area),
            Buffer::empty(Rect::new(0, 0, area.right(), area.bottom()))
        );
    }
}
