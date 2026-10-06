//! The protocol's view of one source's positions.
//!
//! The answers about a source are byte ranges; an editor asks in lines and UTF-16 columns, and
//! this module is where the two meet.
//!
//! The protocol's line model is the client's: `\n`, `\r\n` and `\r` break a line and nothing else
//! does. The compiler's `Lines` also breaks on vertical tab, form feed, NEL, LS and PS, so a source
//! holding one of those is read through the protocol's own model instead of the compiler's index.

use anyhow::{Result, bail};
use lsp_types::{Position, Range};
pub(crate) use tola_typst_syntax::position::utf16_len;
use tola_typst_syntax::position::{
    self as source, Utf16Position, Utf16Range, byte_offset as source_byte_offset,
    checked_position as source_checked_position, utf16_range as source_utf16_range,
};
use tola_typst_syntax::typst_syntax::Lines;

/// The byte offset one position addresses, or an error when it is past the document or its line.
pub(crate) fn byte_offset<T: AsRef<str>>(lines: &Lines<T>, at: Position) -> Result<usize> {
    let text = lines.text();
    // A break after the compiler's own line for this position cannot move it, and one inside that
    // line is reached by the same prefix.
    let reach = lines
        .line_to_range(at.line as usize)
        .map_or(text.len(), |range| range.end);
    if protocol_breaks_differ(first_break(text), reach) {
        return protocol_byte_offset(text, at.line, at.character);
    }
    source_byte_offset(lines, utf16(at))
}

/// The positions that bound one byte range, or `None` when the range is reversed or out of reach.
pub(crate) fn utf16_range<T: AsRef<str>>(
    lines: &Lines<T>,
    bytes: std::ops::Range<usize>,
) -> Option<Range> {
    utf16_range_indexed(lines, bytes, first_break(lines.text()))
}

/// The positions that bound one byte range of a text whose first model-breaking byte is known.
///
/// A caller projecting many ranges through one text — a check publishing every diagnostic of a
/// file — finds that byte once with [`first_break`] instead of rescanning a prefix per range.
pub(crate) fn utf16_range_indexed<T: AsRef<str>>(
    lines: &Lines<T>,
    bytes: std::ops::Range<usize>,
    first_break: Option<usize>,
) -> Option<Range> {
    if bytes.start > bytes.end {
        return None;
    }
    let text = lines.text();
    let reach = bytes.end.min(text.len());
    if !text.is_char_boundary(reach) {
        return None;
    }
    // A break after the range cannot move either of its ends.
    if protocol_breaks_differ(first_break, reach) {
        return Some(protocol_range(Utf16Range::new(
            protocol_position_of(text, bytes.start)?,
            protocol_position_of(text, bytes.end)?,
        )));
    }
    source_utf16_range(lines, bytes).map(protocol_range)
}

/// The position at `line` and `character`, or `None` when either is too large to hold.
pub(crate) fn checked_position(line: usize, character: usize) -> Option<Position> {
    source_checked_position(line, character).map(protocol_position)
}

/// One of the protocol's positions, as the source answers address it.
pub(crate) fn utf16(at: Position) -> Utf16Position {
    Utf16Position::new(at.line, at.character)
}

/// One of the source answers' positions, as the protocol addresses it.
fn protocol_position(at: Utf16Position) -> Position {
    Position::new(at.line, at.character)
}

/// One of the source answers' ranges, as the protocol addresses it.
fn protocol_range(bytes: Utf16Range) -> Range {
    Range::new(protocol_position(bytes.start), protocol_position(bytes.end))
}

/// The first byte of `text` at which the compiler's line model breaks where the protocol's does
/// not.
///
/// The protocol breaks lines on `\n`, `\r\n` and `\r` alone; `typst_syntax::Lines` also breaks on
/// vertical tab, form feed, NEL, LS and PS. A text holding none of those five characters has the
/// same lines in both models, which is what lets a conversion take the compiler's own index; one
/// scan states where the models part, so every later decision is a comparison against the reach.
pub(crate) fn first_break(text: &str) -> Option<usize> {
    text.find(['\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'])
}

/// Whether a text whose first model-breaking byte is `first_break` breaks before `reach`.
fn protocol_breaks_differ(first_break: Option<usize>, reach: usize) -> bool {
    first_break.is_some_and(|at| at < reach)
}

/// The byte offset the protocol addresses at `line` and `character` of `text`.
///
/// A position past the document's lines or past the end of its line is an error, and a character
/// that splits a UTF-16 surrogate pair names no byte.
fn protocol_byte_offset(text: &str, line: u32, character: u32) -> Result<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        let Some(at) = text[start..].find(['\n', '\r']) else {
            bail!("source position line exceeds the document");
        };
        start = break_end(text, start + at);
    }
    let end = text[start..]
        .find(['\n', '\r'])
        .map_or(text.len(), |at| start + at);
    let mut units = 0u32;
    for (at, ch) in text[start..end].char_indices() {
        if units == character {
            return Ok(start + at);
        }
        units += ch.len_utf16() as u32;
        if units > character {
            bail!("source position splits a UTF-16 surrogate pair");
        }
    }
    if units == character {
        Ok(end)
    } else {
        bail!("source position character exceeds the line")
    }
}

/// The protocol position the byte offset `byte` addresses in `text`.
fn protocol_position_of(text: &str, byte: usize) -> Option<Utf16Position> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    let mut line = 0usize;
    let mut start = 0usize;
    while start < byte {
        let Some(at) = text[start..].find(['\n', '\r']) else {
            break;
        };
        let at = start + at;
        let end = break_end(text, at);
        if at >= byte || end > byte {
            break;
        }
        line += 1;
        start = end;
    }
    Some(Utf16Position::new(
        u32::try_from(line).ok()?,
        u32::try_from(source::utf16_len(&text[start..byte])).ok()?,
    ))
}

/// The byte after the line break that starts at `break_start`.
fn break_end(text: &str, break_start: usize) -> usize {
    let mut end = break_start + 1;
    if text.as_bytes()[break_start] == b'\r' && text.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A character the compiler's line model also breaks on stays inside the client's line, in
    /// both directions.
    #[test]
    fn compiler_only_breaks_stay_inside_the_protocol_line() {
        for break_character in ['\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'] {
            let lines = Lines::new(format!("a{break_character}b\r\nc"));
            let b = 1 + break_character.len_utf8();
            assert_eq!(
                byte_offset(&lines, Position::new(0, 3)).unwrap(),
                b + 1,
                "{break_character:?}"
            );
            assert_eq!(
                utf16_range(&lines, b..b + 1),
                Some(Range::new(Position::new(0, 2), Position::new(0, 3))),
                "{break_character:?}"
            );
            assert_eq!(
                byte_offset(&lines, Position::new(1, 1)).unwrap(),
                b + 4,
                "{break_character:?}"
            );
        }
    }

    /// A lone `\r` breaks a protocol line, as it does for the client.
    #[test]
    fn lone_carriage_return_breaks_the_protocol_line() {
        let lines = Lines::new("a\u{0B}b\rc".to_owned());
        assert_eq!(byte_offset(&lines, Position::new(1, 1)).unwrap(), 5);
        assert_eq!(
            utf16_range(&lines, 4..5),
            Some(Range::new(Position::new(1, 0), Position::new(1, 1)))
        );
    }

    /// A break that ends the compiler's line is probed with that line, so a position past the
    /// compiler line's own length is answered when the break ending it is not the protocol's.
    #[test]
    fn position_past_the_compiler_line_is_answered() {
        let lines = Lines::new("x\ny\u{0B}z\nw".to_owned());
        assert_eq!(byte_offset(&lines, Position::new(1, 2)).unwrap(), 4);
        assert_eq!(
            utf16_range(&lines, 4..4),
            Some(Range::new(Position::new(1, 2), Position::new(1, 2)))
        );
    }

    /// The one-scan predicate answers exactly what rescanning the prefix answers, at every reach.
    ///
    /// A text whose first break sits before the reach differs; one whose break sits at or after it
    /// does not, and a text holding no such character never differs.
    #[test]
    fn first_break_decides_projection_by_reach() {
        for text in [
            "plain text\nsecond\n",
            "a\u{0B}b\n正文\n",
            "\u{2028} leading\n",
            "tail \u{85} only\n",
        ] {
            // A reach a conversion can hold is a character boundary; another never occurs.
            for reach in (0..=text.len()).filter(|reach| text.is_char_boundary(*reach)) {
                assert_eq!(
                    protocol_breaks_differ(first_break(text), reach),
                    first_break(&text[..reach]).is_some(),
                    "{text:?} reach {reach}"
                );
            }
        }
    }

    /// A projection handed the break it would find itself answers what rescanning finds.
    #[test]
    fn indexed_projection_matches_rescanning() {
        let lines = Lines::new("a\u{0B}b\nc\n".to_owned());
        let break_at = first_break(lines.text());
        assert_eq!(break_at, Some(1), "the text holds a break");
        for bytes in [0..0, 0..1, 1..2, 2..3, 0..lines.text().len()] {
            assert_eq!(
                utf16_range_indexed(&lines, bytes.clone(), break_at),
                utf16_range(&lines, bytes.clone()),
                "{bytes:?}"
            );
        }
    }

    /// The text an editor holds open: ASCII prose and markup, tabs, Han, emoji, combining marks,
    /// every line ending, and the bytes only the compiler's line model breaks on.
    fn editor_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just("plain ascii text".to_owned()),
                Just("#let title = \"Tola\"".to_owned()),
                Just("\t".to_owned()),
                Just("正文与标点。".to_owned()),
                Just("😀🦊".to_owned()),
                Just("e\u{301}xample".to_owned()),
                Just("\r\n".to_owned()),
                Just("\n".to_owned()),
                Just("\r".to_owned()),
                Just("a\u{0B}b\u{0C}c\u{85}d\u{2028}e\u{2029}f".to_owned()),
                Just(" ".to_owned()),
            ],
            0..12,
        )
        .prop_map(|fragments| fragments.concat())
    }

    /// The protocol's lines of `text`, each without its break, in order.
    fn protocol_lines(text: &str) -> Vec<&str> {
        let bytes = text.as_bytes();
        let mut lines = Vec::new();
        let mut start = 0;
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'\n' => {
                    lines.push(&text[start..at]);
                    at += 1;
                    start = at;
                }
                b'\r' => {
                    lines.push(&text[start..at]);
                    at += if bytes.get(at + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    start = at;
                }
                _ => at += 1,
            }
        }
        lines.push(&text[start..]);
        lines
    }

    /// Whether the boundary at `at` lies between the two bytes of a `\r\n` break, which no
    /// protocol position addresses.
    fn inside_carriage_return_pair(text: &str, at: usize) -> bool {
        at > 0
            && at < text.len()
            && text.as_bytes()[at - 1] == b'\r'
            && text.as_bytes()[at] == b'\n'
    }

    fn char_boundaries(text: &str) -> impl Iterator<Item = usize> + '_ {
        (0..=text.len()).filter(move |at| text.is_char_boundary(*at))
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::Direct(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/target/proptest/position.txt",
                )),
            )),
            cases: 128,
            ..ProptestConfig::default()
        })]

        /// Offsets and positions must agree for the unbounded space an editor sends — any
        /// Unicode text, any character boundary — and a violation hands the editor a position
        /// naming another character, which a table of examples cannot rule out.
        #[test]
        fn protocol_positions_round_trip_addressable_offsets(text in editor_text()) {
            let lines = Lines::new(text.clone());
            let protocol_lines = protocol_lines(&text);
            for at in char_boundaries(&text) {
                let position = utf16_range(&lines, at..at)
                    .expect("a character boundary has a position")
                    .start;
                let recovered = byte_offset(&lines, position);
                if inside_carriage_return_pair(&text, at) {
                    // A byte between `\r` and `\n` addresses no character, so its position is
                    // refused rather than read back as a byte of either line.
                    prop_assert!(recovered.is_err(), "{:?} at {}: {:?}", text, at, position);
                    continue;
                }
                let line = protocol_lines[position.line as usize];
                prop_assert!(
                    position.character as usize <= utf16_len(line),
                    "{:?} at {}: line {:?} holds {} units, position is {:?}",
                    text,
                    at,
                    line,
                    utf16_len(line),
                    position,
                );
                prop_assert_eq!(recovered.ok(), Some(at), "{:?} at {}: {:?}", text, at, position);
            }
        }
    }
}
