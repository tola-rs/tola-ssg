//! Locations and source text retained in diagnostics.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use typst::syntax::Lines;

use crate::{Diagnostic, DiagnosticCode, Severity};

/// A zero-based source position whose character offset counts UTF-16 code units.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePosition {
    pub line: usize,
    pub character: usize,
}

/// An exact, end-exclusive source range from the diagnostic's source revision.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

/// A display path and optional one-based source position.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// Exact UTF-16 range, when the producer retained a complete source span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<SourceRange>,
    pub source_lines: Vec<SourceLine>,
}

impl Location {
    pub(super) fn at_path(path: impl Into<String>) -> Self {
        let path = path.into();
        assert!(!path.is_empty(), "a diagnostic path must not be empty");
        Self {
            path,
            line: None,
            column: None,
            range: None,
            source_lines: Vec::new(),
        }
    }
}

/// A retained window of one source line.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLine {
    pub line: usize,
    /// Zero-based Unicode scalar column of the first retained character.
    pub start_column: usize,
    /// Zero-based UTF-16 column of the first retained character.
    pub start_character: usize,
    /// Whether this window reaches the original line's end.
    pub ends_line: bool,
    pub text: String,
    pub highlight: Option<(usize, usize)>,
}

impl SourceLine {
    pub fn new(line: usize, text: impl Into<String>, highlight: Option<(usize, usize)>) -> Self {
        assert!(line > 0, "a source line number must be one-based");
        let text = text.into();
        let highlight = highlight.filter(|&(start, end)| {
            start < end
                && end <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(end)
        });
        Self {
            line,
            start_column: 0,
            start_character: 0,
            ends_line: true,
            text,
            highlight,
        }
    }
}

/// The written line one span covers, with the highlight that points inside it.
pub struct SourceExcerpt {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub highlight: (usize, usize),
}

impl SourceExcerpt {
    /// The written line one source span covers, with the highlight that points inside it.
    pub fn of(source: &str, span: Range<usize>) -> Option<Self> {
        if span.start > span.end {
            return None;
        }

        let mut start = source.floor_char_boundary(span.start);
        if start == source.len() && source.ends_with('\n') {
            start -= 1;
        }
        let line_start = source[..start].rfind('\n').map_or(0, |offset| offset + 1);
        let line_end = source[start..]
            .find('\n')
            .map_or(source.len(), |offset| start + offset);
        let text = &source[line_start..line_end];
        let column = start - line_start;
        let end = source.ceil_char_boundary(span.end).min(line_end) - line_start;
        let highlight = if end > column {
            (column, end)
        } else if column > 0 {
            (text.floor_char_boundary(column - 1), column)
        } else {
            (0, text.chars().next().map_or(0, char::len_utf8))
        };
        Some(Self {
            line: source[..line_start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
            column: text[..column].chars().count() + 1,
            text: text.to_owned(),
            highlight,
        })
    }
}

/// The exact UTF-16 range one byte span covers, clamped to the text the document holds.
pub(super) fn source_range(source: &str, span: Range<usize>) -> SourceRange {
    SourceRange {
        start: source_position(source, span.start),
        end: source_position(source, span.end),
    }
}

/// The zero-based UTF-16 position one byte offset names.
fn source_position(source: &str, offset: usize) -> SourcePosition {
    let offset = source.floor_char_boundary(offset);
    let line_start = source[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    SourcePosition {
        line: source[..line_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count(),
        character: source[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum(),
    }
}

/// The record for a source whose text the parser rejects.
pub fn unparsable_source(
    code: DiagnosticCode,
    source: &str,
    text: &str,
    offset: Option<usize>,
) -> Diagnostic {
    let diagnostic = Diagnostic::at_path(
        code,
        Severity::Error,
        source.to_owned(),
        format!("`{source}` does not parse as Typst"),
    )
    .with_help("Fix the syntax, or keep the file out of the run with `.ignore`");
    match offset.and_then(|offset| line_column(text, offset)) {
        Some((line, column)) => diagnostic.with_position(line, column),
        None => diagnostic,
    }
}

/// The record for a source whose bytes are not UTF-8 text the reader can format or analyze.
pub fn unreadable_source(code: DiagnosticCode, source: &str) -> Diagnostic {
    Diagnostic::at_path(
        code,
        Severity::Error,
        source.to_owned(),
        format!("Tola could not read `{source}` as UTF-8 text"),
    )
}

/// The one-based line and column a byte offset sits at, when the offset lies inside the text.
pub fn line_column(text: &str, offset: usize) -> Option<(usize, usize)> {
    let (line, column) = Lines::new(text).byte_to_line_column(offset)?;
    Some((line + 1, column + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_highlights() {
        let valid = SourceLine::new(1, "éx", Some((2, 3)));
        let inside_character = SourceLine::new(1, "éx", Some((1, 2)));
        let empty = SourceLine::new(1, "éx", Some((2, 2)));

        assert_eq!(valid.highlight, Some((2, 3)));
        assert_eq!(inside_character.highlight, None);
        assert_eq!(empty.highlight, None);
    }

    #[test]
    fn position_at_eof() {
        assert_eq!(line_column("ab\ncd\n", 3), Some((2, 1)));
        assert_eq!(line_column("ab\n", 3), Some((2, 1)));
        assert_eq!(line_column("ab", 3), None);
    }

    #[test]
    fn excerpt_aligns_highlights() {
        assert!(SourceExcerpt::of("first\nsecond", Range { start: 7, end: 2 }).is_none());
        assert_eq!(SourceExcerpt::of("é", 1..1).unwrap().highlight, (0, 2));
        assert_eq!(SourceExcerpt::of("é", 2..2).unwrap().highlight, (0, 2));
    }
}
