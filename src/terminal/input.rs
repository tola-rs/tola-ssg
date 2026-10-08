//! Text and grapheme-aware editing shared by terminal prompts and full-screen input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub(super) struct InputLine {
    text: String,
    // Byte offset at a grapheme boundary after every edit.
    cursor: usize,
}

impl InputLine {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }

    pub(super) fn into_text(self) -> String {
        self.text
    }

    pub(super) fn paste(&mut self, source: &str) {
        self.insert(&source.replace("\r\n", "\n").replace(['\n', '\r'], " "));
    }

    pub(super) fn insert(&mut self, source: &str) {
        self.text.insert_str(self.cursor, source);
        self.cursor += source.len();
        self.align_cursor();
    }

    pub(super) fn set(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
    }

    fn align_cursor(&mut self) {
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .find(|&byte| byte >= self.cursor)
            .unwrap_or(self.text.len());
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(byte, _)| byte)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |grapheme| self.cursor + grapheme.len())
    }

    pub(super) fn edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor = 0,
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.text.len()
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.replace_range(..self.cursor, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.truncate(self.cursor);
            }
            KeyCode::Backspace => {
                let start = self.previous();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
                self.align_cursor();
            }
            KeyCode::Delete => {
                let end = self.next();
                self.text.replace_range(self.cursor..end, "");
                self.align_cursor();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert(&character.to_string());
            }
            KeyCode::Tab => self.insert("\t"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn editing_respects_grapheme_boundaries() {
        let mut input = InputLine::default();
        input.insert("界👩‍💻e\u{301}");
        input.edit(key(KeyCode::Left));
        input.edit(key(KeyCode::Backspace));
        assert_eq!(input.text, "界e\u{301}");
        assert_eq!(input.cursor, "界".len());
        input.edit(key(KeyCode::Delete));
        assert_eq!(input.text, "界");
        input.edit(key(KeyCode::Home));
        input.insert("前");
        input.edit(key(KeyCode::End));
        assert_eq!(input.cursor, input.text.len());
        assert_eq!(input.text, "前界");
    }

    #[test]
    fn combining_character_stays_whole() {
        let mut input = InputLine::default();
        input.insert("e");
        input.insert("\u{301}");
        input.edit(key(KeyCode::Backspace));
        assert_eq!(input.text, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn character_keys_insert_at_cursor() {
        let mut input = InputLine::default();
        input.edit(key(KeyCode::Char('a')));
        input.edit(key(KeyCode::Char('b')));
        input.edit(key(KeyCode::Left));
        input.edit(key(KeyCode::Char('c')));
        assert_eq!(input.text, "acb");
        assert_eq!(input.cursor, 2);
    }

    #[test]
    fn joined_graphemes_delete_together() {
        let mut line = InputLine::default();
        line.insert("👩💻");
        line.edit(KeyEvent::from(KeyCode::Left));
        line.insert("\u{200d}");
        line.edit(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(line.text(), "");

        line.insert("🇺🇸");
        line.edit(KeyEvent::from(KeyCode::Home));
        line.insert("🇨");
        line.edit(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(line.text(), "🇸");
    }

    #[test]
    fn paste_keeps_the_line_single() {
        let mut line = InputLine::default();
        line.paste("one\r\ntwo\nthree\rfour");
        assert_eq!(line.text(), "one two three four");
    }

    #[test]
    fn control_keys_edit_line_segments() {
        let mut input = InputLine::default();
        input.insert("前abc後");
        input.edit(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        input.edit(key(KeyCode::Right));
        input.edit(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(input.text(), "abc後");
        input.edit(key(KeyCode::Right));
        input.edit(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(input.text(), "a");
        input.edit(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        input.edit(key(KeyCode::Char('b')));
        assert_eq!(input.text(), "ab");
    }
}
