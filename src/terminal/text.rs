//! Display external text without granting it terminal control.

use super::ansi;
use super::ansi::push_visible;

/// Preserve line breaks and tabs while making other controls visible.
///
/// Text Tola did not write arrives as a command wrote it to a terminal, escape sequences
/// included. Those sequences are dropped rather than drawn: `\x1b[32m` colors nothing in a
/// diagnostic note, and spelling it out buries the output the reader came for. Every control
/// that remains is spelled out, so displayed text still cannot move a cursor.
pub(crate) fn multiline(source: &str) -> String {
    ansi::paint(source, false)
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
}
