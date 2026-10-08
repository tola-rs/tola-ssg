//! The one input line: an incremental filter, a prompt, or an export path.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::terminal::input::InputLine;
use crate::terminal::style::Palette;
use crate::terminal::text;

/// What the input line did with one key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    /// The line is still being edited.
    Editing,
    /// The reader accepted the text; read it with [`TextLine::text`].
    Accepted,
    /// The reader dismissed the line; the caller keeps what it had.
    Dismissed,
}

/// The layer's one line of editable text.
pub(crate) struct TextLine {
    label: String,
    input: InputLine,
}

impl TextLine {
    /// Opens an empty line under `label`, named as the hint shows it.
    pub(crate) fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            input: InputLine::default(),
        }
    }

    /// The line as it stands.
    pub(crate) fn text(&self) -> &str {
        self.input.text()
    }

    /// Answers one key press: Enter accepts, Esc dismisses, everything else edits the line.
    pub(crate) fn key(&mut self, key: &KeyEvent) -> Reply {
        match key.code {
            KeyCode::Enter => Reply::Accepted,
            KeyCode::Esc => Reply::Dismissed,
            KeyCode::Tab => Reply::Editing,
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => Reply::Editing,
            _ => {
                self.input.edit(*key);
                Reply::Editing
            }
        }
    }

    /// Inserts pasted text as one edit; its line breaks become spaces.
    pub(crate) fn paste(&mut self, text: &str) {
        self.input.paste(text);
    }

    /// Draws the labelled line with its cursor inside `area`.
    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect, palette: Palette) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let columns = area.width as usize;
        let label = format!("{}: ", self.label);
        let label = if label.width() + 1 < columns {
            label
        } else {
            String::new()
        };
        let visible = columns.saturating_sub(label.width()).saturating_sub(1);
        let focused = text::single_line(self.input.before_cursor()).width();
        let displayed = text::single_line(self.input.text());
        let window = text::window(&displayed, visible, focused);
        let cursor = label.width() + focused.saturating_sub(window.start_column);
        let line = Line::from(vec![
            Span::styled(label.clone(), palette.accent_style()),
            Span::styled(window.text.to_owned(), palette.selected_style()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        let cursor = area.x + u16::try_from(cursor).unwrap_or(u16::MAX);
        frame.set_cursor_position(Position::new(cursor.min(area.right() - 1), area.y));
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn typed(line: &mut TextLine, source: &str) {
        for character in source.chars() {
            assert_eq!(
                line.key(&KeyEvent::from(KeyCode::Char(character))),
                Reply::Editing
            );
        }
    }

    #[test]
    fn typing_edits_the_line() {
        let mut line = TextLine::new("filter");
        typed(&mut line, "caf");
        line.key(&KeyEvent::from(KeyCode::Backspace));
        typed(&mut line, "é");
        assert_eq!(line.text(), "caé");
    }

    #[test]
    fn enter_accepts_the_text() {
        let mut line = TextLine::new("filter");
        typed(&mut line, "tag");
        assert_eq!(line.key(&KeyEvent::from(KeyCode::Enter)), Reply::Accepted);
        assert_eq!(line.text(), "tag");
    }

    #[test]
    fn escape_dismisses_the_text() {
        let mut line = TextLine::new("filter");
        typed(&mut line, "tag");
        assert_eq!(line.key(&KeyEvent::from(KeyCode::Esc)), Reply::Dismissed);
        assert_eq!(line.text(), "tag");
    }

    #[test]
    fn end_cursor_follows_visible_text() {
        let mut line = TextLine::new("filter");
        typed(&mut line, "abcdefgh");
        let mut terminal = Terminal::new(TestBackend::new(12, 1)).unwrap();
        terminal
            .draw(|frame| line.draw(frame, frame.area(), Palette::new(false)))
            .unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[cursor].symbol(), " ");
        assert_eq!(buffer[Position::new(cursor.x - 1, cursor.y)].symbol(), "h");
    }

    #[test]
    fn escaped_text_keeps_cursor_aligned() {
        for source in ["a\tb", "a\u{1b}b"] {
            let mut line = TextLine::new("filter");
            line.paste(source);
            let mut terminal = Terminal::new(TestBackend::new(30, 1)).unwrap();
            terminal
                .draw(|frame| line.draw(frame, frame.area(), Palette::new(false)))
                .unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[cursor].symbol(), " ");
            assert_eq!(buffer[Position::new(cursor.x - 1, cursor.y)].symbol(), "b");
        }
    }

    #[test]
    fn narrow_input_keeps_text_visible() {
        let mut line = TextLine::new("filter");
        line.paste("abcdef");
        for width in 2..=9 {
            let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
            terminal
                .draw(|frame| line.draw(frame, frame.area(), Palette::new(false)))
                .unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width);
            assert_eq!(terminal.backend().buffer()[cursor].symbol(), " ");
            assert_eq!(
                terminal.backend().buffer()[Position::new(cursor.x - 1, 0)].symbol(),
                "f"
            );
        }
    }
}
