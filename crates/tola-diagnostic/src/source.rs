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
    #[serde(skip)]
    #[cfg_attr(feature = "json-schema", schemars(skip))]
    pub file: Option<std::path::PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<SourceRange>,
    pub source_lines: Vec<SourceLine>,
}

impl Location {
    pub fn editor_file(&self, root: &std::path::Path) -> Option<std::path::PathBuf> {
        if let Some(file) = &self.file {
            return Some(root.join(file));
        }
        (!self.path.starts_with('@')).then(|| root.join(&self.path))
    }

    pub(super) fn at_path(path: impl Into<String>) -> Self {
        let path = path.into();
        assert!(!path.is_empty(), "a diagnostic path must not be empty");
        Self {
            path,
            file: None,
            line: None,
            column: None,
            range: None,
            source_lines: Vec::new(),
        }
    }
}

/// Retained window of one source line.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLine {
    pub line: usize,
    pub start_column: usize,
    pub start_character: usize,
    pub ends_line: bool,
    pub text: String,
    pub highlight: Option<(usize, usize)>,
}

impl SourceLine {
    pub fn new(line: usize, text: impl Into<String>, hl: Option<(usize, usize)>) -> Self {
        assert!(line > 0, "a source line number must be one-based");
        let text = text.into();
        let highlight = hl.filter(|&(start, end)| {
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

pub struct SourceExcerpt {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub highlight: (usize, usize),
}

impl SourceExcerpt {
    pub fn of(src: &str, span: Range<usize>) -> Option<Self> {
        if span.start > span.end {
            return None;
        }

        let lines = Lines::new(src);
        let mut start = src.floor_char_boundary(span.start);
        let mut line = lines.byte_to_line(start)?;
        if start == src.len() && line > 0 && lines.line_to_byte(line)? == start {
            line -= 1;
            start = src.floor_char_boundary(start - 1);
        }
        let range = lines.line_to_range(line)?;
        let line_start = range.start;
        let text = src[range].trim_end_matches(typst::syntax::is_newline);
        let line_end = line_start + text.len();
        let start = start.min(line_end);
        let col = start - line_start;
        let end = src.ceil_char_boundary(span.end).min(line_end) - line_start;
        let highlight = if end > col {
            (col, end)
        } else if col > 0 {
            (text.floor_char_boundary(col - 1), col)
        } else {
            (0, text.chars().next().map_or(0, char::len_utf8))
        };
        Some(Self {
            line: line + 1,
            column: text[..col].chars().count() + 1,
            text: text.to_owned(),
            highlight,
        })
    }
}

pub(super) fn source_range(src: &str, span: Range<usize>) -> SourceRange {
    let lines = Lines::new(src);
    SourceRange {
        start: source_position(&lines, span.start),
        end: source_position(&lines, span.end),
    }
}

fn source_position(lines: &Lines<&str>, offset: usize) -> SourcePosition {
    let src = lines.text();
    let offset = src.floor_char_boundary(offset);
    let line = lines
        .byte_to_line(offset)
        .expect("the offset is in the source");
    let line_start = lines.line_to_byte(line).expect("the offset's line exists");
    SourcePosition {
        line,
        character: src[line_start..offset].chars().map(char::len_utf16).sum(),
    }
}

pub fn unparsable_source(
    code: DiagnosticCode,
    src: &str,
    text: &str,
    offset: Option<usize>,
) -> Diagnostic {
    let diag = Diagnostic::at_path(
        code,
        Severity::Error,
        src.to_owned(),
        format!("`{src}` does not parse as Typst"),
    )
    .with_help("Fix the syntax, or keep the file out of the run with `.ignore`");
    match offset.and_then(|off| line_column(text, off)) {
        Some((line, col)) => diag.with_position(line, col),
        None => diag,
    }
}

pub fn unreadable_source(code: DiagnosticCode, src: &str) -> Diagnostic {
    Diagnostic::at_path(
        code,
        Severity::Error,
        src.to_owned(),
        format!("Tola could not read `{src}` as UTF-8 text"),
    )
}

pub fn line_column(text: &str, offset: usize) -> Option<(usize, usize)> {
    let (line, col) = Lines::new(text).byte_to_line_column(offset)?;
    Some((line + 1, col + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_highlights() {
        let valid = SourceLine::new(1, "éx", Some((2, 3)));
        let mid_char = SourceLine::new(1, "éx", Some((1, 2)));
        let empty = SourceLine::new(1, "éx", Some((2, 2)));

        assert_eq!(valid.highlight, Some((2, 3)));
        assert_eq!(mid_char.highlight, None);
        assert_eq!(empty.highlight, None);
    }

    #[test]
    fn position_at_eof() {
        assert_eq!(line_column("ab\ncd\n", 3), Some((2, 1)));
        assert_eq!(line_column("ab\n", 3), Some((2, 1)));
        assert_eq!(line_column("ab", 3), None);
    }

    #[test]
    fn excerpt_highlights() {
        assert!(SourceExcerpt::of("first\nsecond", Range { start: 7, end: 2 }).is_none());
        assert_eq!(SourceExcerpt::of("é", 1..1).unwrap().highlight, (0, 2));
        assert_eq!(SourceExcerpt::of("é", 2..2).unwrap().highlight, (0, 2));
    }
}
