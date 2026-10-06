//! Display external text without granting it terminal control.

use std::fmt::Write;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Preserve line breaks and tabs while making other controls visible.
///
/// Text Tola did not write arrives as a command wrote it to a terminal, escape sequences
/// included. Those sequences are dropped rather than drawn: `\x1b[32m` colors nothing in a
/// diagnostic note, and spelling it out buries the output the reader came for. Every control
/// that remains is spelled out, so displayed text still cannot move a cursor.
pub(crate) fn multiline(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            skip_escape(&mut characters);
            continue;
        }
        if character == '\r' && characters.peek() == Some(&'\n') {
            continue;
        }
        if matches!(character, '\n' | '\t') {
            output.push(character);
        } else {
            push_visible(&mut output, character);
        }
    }
    output
}

/// Consume one escape sequence, the introducer already read.
///
/// A sequence whose final byte never arrives takes the rest of the text with it: a half-read
/// escape is still control text, and drawing what follows would be guessing where it ended.
fn skip_escape(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match characters.peek() {
        // Control Sequence Introducer, and its two-byte C1 spelling.
        Some('[') | Some('\u{9b}') => {
            characters.next();
            for character in characters.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&character) {
                    break;
                }
            }
        }
        Some(']') => {
            characters.next();
            for character in characters.by_ref() {
                match character {
                    '\u{7}' => break,
                    '\u{1b}' => {
                        if characters.peek() == Some(&'\\') {
                            characters.next();
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
        _ => {
            characters.next();
        }
    }
}

/// Join `text`'s lines as one block, the first line flush and every later line indented one
/// display unit.
///
/// A carriage return left at a later line's end is dropped: on a terminal it would move the
/// cursor back over the indent the block just wrote.
pub(crate) fn indent_continuation_lines(text: &str) -> String {
    let mut lines = text.lines();
    let mut output = lines.next().unwrap_or_default().to_owned();
    for line in lines {
        output.push('\n');
        super::append_indent(&mut output, 1);
        output.push_str(line.trim_end_matches('\r'));
    }
    output
}

/// Split one chunk of a hook's output into the display fragments it produces.
///
/// A hook must not control the terminal, so a carriage return becomes visible text rather than a
/// cursor move: a chunk that continues a returned line starts with one, and a chunk that ends one
/// keeps it pending in `carriage_return` until the next chunk decides whether it was a line break.
/// Each fragment either ends with a line break or is the line the hook is still writing.
pub(crate) fn hook_fragments(source: &str, carriage_return: &mut bool) -> Vec<String> {
    let mut displayed = String::new();
    let mut source = source;
    if *carriage_return {
        match source.strip_prefix('\n') {
            Some(rest) => {
                displayed.push('\n');
                source = rest;
            }
            None => displayed.push_str("\\r"),
        }
    }
    *carriage_return = source.ends_with('\r');
    if *carriage_return {
        source = &source[..source.len() - 1];
    }
    displayed.push_str(&multiline(source));
    displayed.split_inclusive('\n').map(str::to_owned).collect()
}

/// Paths and identifiers occupy one line, even when their contents contain controls.
pub(crate) fn single_line(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    for character in source.chars() {
        push_visible(&mut output, character);
    }
    output
}

/// Source text drawn where it sits, with every control character but `\n` and `\t` replaced by a
/// printable placeholder of the same UTF-8 length.
///
/// A snippet has offsets into its own text, so a replacement that changed a character's length
/// would move every position after it. Tabs stay because the layout expands them, and line breaks
/// stay because they are what the reader counts lines by; controls are one or two bytes long (C0,
/// DEL, and C1), so one of the two placeholders always has the length of the character it replaces.
pub(crate) fn visible_in_place(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    for character in source.chars() {
        if !character.is_control() || matches!(character, '\n' | '\t') {
            output.push(character);
            continue;
        }
        output.push(if character.len_utf8() == 1 { '?' } else { '·' });
    }
    output
}

pub(crate) struct DisplayWindow<'a> {
    pub(crate) text: &'a str,
    pub(crate) start_column: usize,
}

/// Keep a display position visible without splitting an extended grapheme.
pub(crate) fn window(source: &str, columns: usize, focus: usize) -> DisplayWindow<'_> {
    let total = source.width();
    let desired = focus
        .saturating_sub(columns / 3)
        .min(total.saturating_sub(columns));
    let mut start_byte = 0;
    let mut start_column = 0;
    for (byte, grapheme) in source.grapheme_indices(true) {
        let next = start_column + grapheme.width();
        if desired == 0 || next > desired {
            break;
        }
        start_byte = byte + grapheme.len();
        start_column = next;
    }
    let mut end_byte = start_byte;
    let mut end_column = start_column;
    for (byte, grapheme) in source[start_byte..].grapheme_indices(true) {
        let next = end_column + grapheme.width();
        if next > start_column + columns {
            break;
        }
        end_byte = start_byte + byte + grapheme.len();
        end_column = next;
    }
    DisplayWindow {
        text: &source[start_byte..end_byte],
        start_column,
    }
}

fn push_visible(output: &mut String, character: char) {
    match character {
        '\n' => output.push_str("\\n"),
        '\r' => output.push_str("\\r"),
        '\t' => output.push_str("\\t"),
        character if character.is_ascii_control() => {
            write!(output, "\\x{:02x}", character as u32).expect("writing to a String succeeds");
        }
        character if character.is_control() => {
            write!(output, "\\u{{{:x}}}", character as u32).expect("writing to a String succeeds");
        }
        character => output.push(character),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_keeps_layout_without_controls() {
        assert_eq!(
            multiline("first\r\n\tsecond\rreplaced\u{7}\u{85}"),
            "first\n\tsecond\\rreplaced\\x07\\u{85}"
        );
    }

    #[test]
    fn multiline_drops_escape_sequences() {
        assert_eq!(
            multiline("a\u{1b}[32mgreen\u{1b}[0m b\u{1b}]0;title\u{7}c"),
            "agreen bc",
            "the operating-system sequence takes its own terminator"
        );
        assert_eq!(
            multiline("x\u{1b}[38;5;248my"),
            "xy",
            "a sequence's final byte ends it"
        );
        assert_eq!(
            multiline("cut\u{1b}"),
            "cut",
            "an unfinished sequence ends text"
        );
        assert_eq!(multiline("\u{1b}[m+\u{7}"), "+\\x07");
    }

    #[test]
    fn single_line_holds_one_row() {
        assert_eq!(single_line("content/a\n\tb.typ"), "content/a\\n\\tb.typ");
        assert_eq!(single_line("界 👩‍💻 e\u{301}"), "界 👩‍💻 e\u{301}");
    }

    #[test]
    fn control_replacement_keeps_offsets() {
        let source = "a\u{1b}[31m\u{7}b\t\n\u{85}c";
        let visible = visible_in_place(source);
        assert_eq!(visible, "a?[31m?b\t\n·c");
        assert_eq!(visible.len(), source.len());
        assert!(
            !visible
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        );
    }

    #[test]
    fn window_clamps_focus_past_text_end() {
        let end = window("abcdef", 3, 6);
        assert_eq!(end.text, "def");
        assert_eq!(end.start_column, 3);
        let past = window("abcdef", 3, 99);
        assert_eq!(past.text, "def");
        assert_eq!(past.start_column, 3);
        let empty = window("", 80, 5);
        assert_eq!(empty.text, "");
        assert_eq!(empty.start_column, 0);
    }

    #[test]
    fn window_keeps_wide_grapheme_whole() {
        // The focus falls inside the wide grapheme, so the window starts at the grapheme rather
        // than at the column that would split it.
        let focused = window("a界x", 2, 2);
        assert_eq!(focused.text, "界");
        assert_eq!(focused.start_column, 1);
    }
}
