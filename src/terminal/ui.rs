//! Drawing and keys for the development status screen.

use std::io;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::layout::{Rect, Size};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use super::style::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    PreviousRound,
    NextRound,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    Copy,
}

pub(crate) fn cancels(key: &KeyEvent) -> bool {
    (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        || (key.code == KeyCode::Char('q') && key.modifiers.is_empty())
}

pub(crate) struct Screen<B: Backend<Error = io::Error>> {
    terminal: Terminal<B>,
    size: Size,
}

impl<B: Backend<Error = io::Error>> Screen<B> {
    pub(crate) fn new(backend: B, width: u16, height: u16) -> io::Result<Self> {
        Ok(Self {
            terminal: Terminal::with_options(
                backend,
                TerminalOptions {
                    viewport: Viewport::Fixed(Rect::new(0, 0, width, height)),
                },
            )?,
            size: Size::new(width, height),
        })
    }

    pub(crate) fn draw_frame(
        &mut self,
        size: Size,
        draw: impl FnOnce(&mut Frame<'_>),
    ) -> io::Result<()> {
        if self.size != size {
            self.terminal
                .resize(Rect::new(0, 0, size.width, size.height))?;
            self.size = size;
        }
        self.terminal.draw(draw)?;
        Ok(())
    }
}

pub(crate) mod keymap {
    use super::{Action, KeyCode, KeyEvent, KeyModifiers};

    /// The action one development key asks for, when it asks for one.
    pub(crate) fn action(key: &KeyEvent) -> Option<Action> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        Some(match key.code {
            KeyCode::Up | KeyCode::Char('k') => Action::Up,
            KeyCode::Down | KeyCode::Char('j') => Action::Down,
            KeyCode::PageUp => Action::PageUp,
            KeyCode::PageDown => Action::PageDown,
            KeyCode::Home => Action::First,
            KeyCode::End => Action::Last,
            KeyCode::Left | KeyCode::Char('[') => Action::PreviousRound,
            KeyCode::Right | KeyCode::Char(']') => Action::NextRound,
            KeyCode::Char('c') => Action::Copy,
            _ => return None,
        })
    }
}

pub(crate) mod hints {
    use super::{Action, Frame, Palette, Paragraph, Rect};

    pub(crate) fn draw(
        frame: &mut Frame<'_>,
        area: Rect,
        notice: &str,
        live: &[Action],
        palette: Palette,
    ) {
        if area.height == 0 {
            return;
        }
        let mut parts = Vec::new();
        if live.contains(&Action::PreviousRound) || live.contains(&Action::NextRound) {
            parts.push("←/→ rounds");
        }
        if live.contains(&Action::Up) || live.contains(&Action::Down) {
            parts.push("↑/↓ scroll");
        }
        if live.contains(&Action::Copy) {
            parts.push("c copy");
        }
        parts.push("Ctrl-C quit");
        let text = if notice.is_empty() {
            parts.join("  ")
        } else {
            format!("{notice}\n{}", parts.join("  "))
        };
        frame.render_widget(Paragraph::new(text).style(palette.dim_style()), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_keys_cover_view() {
        let action = |code| keymap::action(&KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(action(KeyCode::Left), Some(Action::PreviousRound));
        assert_eq!(action(KeyCode::Right), Some(Action::NextRound));
        assert_eq!(action(KeyCode::Down), Some(Action::Down));
        assert_eq!(action(KeyCode::Char('c')), Some(Action::Copy));
        assert!(cancels(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }
}
