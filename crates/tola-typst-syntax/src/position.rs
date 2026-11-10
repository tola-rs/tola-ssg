//! Checked conversion between byte offsets and the positions an editor addresses text by.

use typst_syntax::Lines;

/// Why a position addresses no byte of the source.
#[derive(Debug, thiserror::Error)]
pub enum PositionError {
    /// The line is past the document's last line.
    #[error("source position line exceeds the document")]
    LineOutOfRange,
    /// The column falls inside a surrogate pair.
    #[error("source position splits a UTF-16 surrogate pair")]
    SplitsSurrogatePair,
    /// The column is past the end of its line.
    #[error("source position character exceeds the line")]
    PastLineEnd,
}

/// One position in a source: a line, and a column counted in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf16Position {
    /// The line, counted from zero.
    pub line: u32,
    /// The column, counted from zero in UTF-16 code units.
    pub character: u32,
}

impl Utf16Position {
    /// The position at `line` and `character`.
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// The span between two positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Utf16Range {
    /// Where the span starts.
    pub start: Utf16Position,
    /// Where the span ends.
    pub end: Utf16Position,
}

impl Utf16Range {
    /// The span from `start` to `end`.
    pub fn new(start: Utf16Position, end: Utf16Position) -> Self {
        Self { start, end }
    }
}

/// The byte offset one position addresses, or an error when the position is past the
/// document's lines or past the end of its line.
pub fn byte_offset<T: AsRef<str>>(
    lines: &Lines<T>,
    position: Utf16Position,
) -> Result<usize, PositionError> {
    let line = lines
        .line_to_range(position.line as usize)
        .ok_or(PositionError::LineOutOfRange)?;
    let text = lines.text()[line.clone()].trim_end_matches(typst_syntax::is_newline);
    let column = position.character as usize;
    let mut utf16 = 0;
    for (offset, character) in text.char_indices() {
        if utf16 == column {
            return Ok(line.start + offset);
        }
        utf16 += character.len_utf16();
        if utf16 > column {
            return Err(PositionError::SplitsSurrogatePair);
        }
    }
    if utf16 == column {
        Ok(line.start + text.len())
    } else {
        Err(PositionError::PastLineEnd)
    }
}

/// The positions that bound one byte range, or `None` when the range is reversed or reaches
/// past the source.
pub fn utf16_range<T: AsRef<str>>(
    lines: &Lines<T>,
    bytes: std::ops::Range<usize>,
) -> Option<Utf16Range> {
    if bytes.start > bytes.end {
        return None;
    }
    let convert = |byte| {
        let line = lines.byte_to_line(byte)?;
        let start = lines.line_to_byte(line)?;
        let character = utf16_len(lines.text().get(start..byte)?);
        checked_position(line, character)
    };
    Some(Utf16Range::new(convert(bytes.start)?, convert(bytes.end)?))
}

/// The UTF-16 code-unit length of `text`, which is the LSP column of its end.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The position at `line` and `character`, or `None` when either is too large for the
/// coordinates an editor carries.
pub fn checked_position(line: usize, character: usize) -> Option<Utf16Position> {
    Some(Utf16Position::new(
        line.try_into().ok()?,
        character.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns_map_to_byte_offsets() {
        let lines = Lines::new("a🦊b\r\n正文\n");
        assert_eq!(byte_offset(&lines, Utf16Position::new(0, 3)).unwrap(), 5);
        assert_eq!(byte_offset(&lines, Utf16Position::new(0, 4)).unwrap(), 6);
        assert!(byte_offset(&lines, Utf16Position::new(0, 2)).is_err());
        assert!(byte_offset(&lines, Utf16Position::new(0, 5)).is_err());
        assert_eq!(byte_offset(&lines, Utf16Position::new(2, 0)).unwrap(), 15);
        assert!(byte_offset(&lines, Utf16Position::new(2, 1)).is_err());
        assert!(byte_offset(&lines, Utf16Position::new(3, 0)).is_err());
    }
}
