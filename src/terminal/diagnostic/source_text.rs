//! Retained source windows mapped to diagnostic renderer rows.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use codespan_reporting::files::{Error as FilesError, Files};
use tola_build::diagnostic::{Location, SourceLine, SourcePosition, SourceRange};

use crate::terminal::source::SourceFiles;
use crate::terminal::text;

use super::{DiagnosticText, LocationText};

pub(super) struct RenderedFiles<'a> {
    texts: BTreeMap<&'a str, Text>,
}

impl<'a> RenderedFiles<'a> {
    pub(super) fn of(escaped: &'a DiagnosticText<'_>, sources: Option<&SourceFiles>) -> Self {
        let mut windows: BTreeMap<&str, Vec<&SourceLine>> = BTreeMap::new();
        for located in escaped.locations() {
            if let (Some(path), Some(location)) = (located.path(), located.location) {
                windows
                    .entry(path)
                    .or_default()
                    .extend(&location.source_lines);
            }
        }
        let mut texts = BTreeMap::new();
        for (path, lines) in windows {
            if !lines.is_empty() {
                texts.insert(path, Text::of_windows(&lines));
            }
        }
        for located in escaped.locations() {
            let (Some(path), Some(location)) = (located.path(), located.location) else {
                continue;
            };
            // A compiled excerpt owns the entire file view, including what it omitted.
            if !texts.contains_key(path)
                && let Some(source) = sources.and_then(|sources| sources.read(&location.path))
            {
                texts.insert(path, Text::of(source));
            }
        }
        Self { texts }
    }

    pub(super) fn compact_span(&self, located: &LocationText<'_>) -> Option<String> {
        self.texts
            .get(located.path()?)?
            .compact_span(located.location?)
    }

    pub(super) fn is_span_shortened(&self, located: &LocationText<'_>) -> bool {
        let Some(range) = located.location.and_then(|location| location.range) else {
            return false;
        };
        self.texts
            .get(located.path().unwrap_or_default())
            .is_some_and(|text| !text.span_is_complete(range))
    }

    pub(super) fn range(&self, located: &LocationText<'_>) -> Option<Range<usize>> {
        self.texts.get(located.path()?)?.range(located.location?)
    }
}

struct SourceWindow {
    line: usize,
    start_column: usize,
    start_character: usize,
    ends_line: bool,
    text: String,
}

struct SourceRow {
    line: usize,
    start_column: usize,
    start_character: usize,
    ends_line: bool,
    bytes: Range<usize>,
}

struct Text {
    text: Arc<str>,
    rows: Vec<SourceRow>,
}

impl Text {
    fn of(source: Arc<str>) -> Self {
        let windows = source
            .split('\n')
            .enumerate()
            .map(|(line, source)| SourceWindow {
                line: line + 1,
                start_column: 0,
                start_character: 0,
                ends_line: true,
                text: text::visible_in_place(source.trim_end_matches('\r')),
            })
            .collect();
        Self::of_rows(windows)
    }

    fn of_windows(lines: &[&SourceLine]) -> Self {
        let mut windows = lines
            .iter()
            .map(|line| SourceWindow {
                line: line.line,
                start_column: line.start_column,
                start_character: line.start_character,
                ends_line: line.ends_line,
                text: text::visible_in_place(&line.text),
            })
            .collect::<Vec<_>>();
        windows.sort_by_key(|window| (window.line, window.start_character));
        let mut merged: Vec<SourceWindow> = Vec::new();
        for window in windows {
            if let Some(previous) = merged.last_mut()
                && previous.line == window.line
                && let Some(overlap) = window.start_character.checked_sub(previous.start_character)
                && let Some(overlap_byte) = utf16_byte_offset(&previous.text, overlap)
            {
                let retained_units = previous.text[overlap_byte..].encode_utf16().count();
                if let Some(append_from) = utf16_byte_offset(&window.text, retained_units) {
                    previous.text.push_str(&window.text[append_from..]);
                }
                previous.ends_line |= window.ends_line;
                continue;
            }
            merged.push(window);
        }
        Self::of_rows(merged)
    }

    fn of_rows(windows: Vec<SourceWindow>) -> Self {
        let mut text = String::new();
        let mut rows = Vec::with_capacity(windows.len());
        for window in windows {
            if !rows.is_empty() {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(&window.text);
            rows.push(SourceRow {
                line: window.line,
                start_column: window.start_column,
                start_character: window.start_character,
                ends_line: window.ends_line,
                bytes: start..text.len(),
            });
        }
        Self {
            text: Arc::from(text),
            rows,
        }
    }

    fn offset(&self, position: SourcePosition) -> Option<usize> {
        self.rows.iter().find_map(|row| {
            if row.line != position.line + 1 {
                return None;
            }
            let character = position.character.checked_sub(row.start_character)?;
            Some(row.bytes.start + utf16_byte_offset(&self.text[row.bytes.clone()], character)?)
        })
    }

    fn endpoint(&self, position: SourcePosition) -> Option<usize> {
        if position.character == 0 {
            return self
                .rows
                .iter()
                .rev()
                .find(|row| row.line == position.line && row.ends_line)
                .map(|row| row.bytes.end);
        }
        self.offset(position)
    }

    fn row_index(&self, byte: usize) -> usize {
        self.rows
            .partition_point(|row| row.bytes.start <= byte)
            .saturating_sub(1)
    }

    fn range(&self, location: &Location) -> Option<Range<usize>> {
        if let Some(range) = location.range {
            let start = self.offset(range.start)?;
            if range.start == range.end {
                return Some(start..start);
            }
            let end = self
                .endpoint(range.end)
                .unwrap_or_else(|| self.rows[self.row_index(start)].bytes.end);
            return Some(start..end.max(start));
        }
        let line = location.line?;
        if let Some(source) = location
            .source_lines
            .iter()
            .find(|source| source.line == line && source.highlight.is_some())
        {
            let (start, end) = source.highlight?;
            let character =
                |byte| source.start_character + source.text[..byte].encode_utf16().count();
            return Some(
                self.offset(SourcePosition {
                    line: line - 1,
                    character: character(start),
                })?..self.offset(SourcePosition {
                    line: line - 1,
                    character: character(end),
                })?,
            );
        }
        let column = location.column?.saturating_sub(1);
        self.rows.iter().find_map(|row| {
            if row.line != line {
                return None;
            }
            let local = column.checked_sub(row.start_column)?;
            let line_text = &self.text[row.bytes.clone()];
            let byte = line_text
                .char_indices()
                .map(|(byte, _)| byte)
                .chain([line_text.len()])
                .nth(local)?;
            let start = row.bytes.start + byte;
            Some(start..start)
        })
    }

    fn compact_span(&self, location: &Location) -> Option<String> {
        let Some(range) = location.range else {
            return Some(text::single_line(&self.text[self.range(location)?]));
        };
        let start = self.offset(range.start)?;
        let first_row = self.row_index(start);
        let mut compact = String::new();
        let last_line = range.end.line.saturating_sub(usize::from(
            range.end.line > range.start.line && range.end.character == 0,
        ));
        if range.start.line == last_line {
            let end = if range.start.line == range.end.line {
                self.offset(range.end)
            } else {
                self.endpoint(range.end)
            };
            let last_row = end.map(|byte| self.row_index(byte));
            for row in first_row..=last_row.unwrap_or(first_row) {
                if row > first_row {
                    compact.push('…');
                }
                let window = &self.rows[row];
                let start = if row == first_row {
                    start
                } else {
                    window.bytes.start
                };
                let end = if Some(row) == last_row {
                    end.unwrap()
                } else {
                    window.bytes.end
                };
                compact.push_str(&text::single_line(&self.text[start..end]));
            }
            if end.is_none() {
                compact.push('…');
            }
            return Some(compact);
        }
        compact.push_str(&text::single_line(
            &self.text[start..self.rows[first_row].bytes.end],
        ));
        let Some(end) = self.endpoint(range.end) else {
            compact.push('…');
            return Some(compact);
        };
        let last_row = self.row_index(end);
        if self.rows[last_row].line > self.rows[first_row].line
            && let Some(character) = self.text[self.rows[last_row].bytes.start..end]
                .chars()
                .next_back()
                .filter(|character| !character.is_whitespace())
        {
            compact.push('…');
            compact.push_str(&text::single_line(&character.to_string()));
        }
        Some(compact)
    }

    fn span_is_complete(&self, range: SourceRange) -> bool {
        if range.start == range.end {
            return self.offset(range.start).is_some();
        }
        let (Some(start), Some(end)) = (self.offset(range.start), self.endpoint(range.end)) else {
            return false;
        };
        self.rows[self.row_index(start)..=self.row_index(end.max(start))]
            .windows(2)
            .all(|rows| {
                if rows[0].line == rows[1].line {
                    rows[0].start_character
                        + self.text[rows[0].bytes.clone()].encode_utf16().count()
                        == rows[1].start_character
                } else {
                    rows[0].ends_line
                        && rows[0].line + 1 == rows[1].line
                        && rows[1].start_character == 0
                }
            })
    }
}

fn utf16_byte_offset(text: &str, character: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, scalar) in text.char_indices() {
        if units == character {
            return Some(byte);
        }
        units += scalar.len_utf16();
    }
    (units == character).then_some(text.len())
}

impl<'a> Files<'a> for RenderedFiles<'a> {
    type FileId = &'a str;
    type Name = &'a str;
    type Source = &'a str;

    fn name(&'a self, id: &'a str) -> Result<&'a str, FilesError> {
        Ok(id)
    }

    fn source(&'a self, id: &'a str) -> Result<&'a str, FilesError> {
        self.texts
            .get(id)
            .map(|text| text.text.as_ref())
            .ok_or(FilesError::FileMissing)
    }

    fn line_index(&'a self, id: &'a str, byte: usize) -> Result<usize, FilesError> {
        let text = self.texts.get(id).ok_or(FilesError::FileMissing)?;
        Ok(text.row_index(byte))
    }

    fn line_number(&'a self, id: &'a str, line: usize) -> Result<usize, FilesError> {
        let text = self.texts.get(id).ok_or(FilesError::FileMissing)?;
        text.rows
            .get(line)
            .map(|row| row.line)
            .ok_or(FilesError::LineTooLarge {
                given: line,
                max: text.rows.len(),
            })
    }

    // Typst CLI columns count Unicode scalars from zero.
    fn column_number(&'a self, id: &'a str, line: usize, byte: usize) -> Result<usize, FilesError> {
        let text = self.texts.get(id).ok_or(FilesError::FileMissing)?;
        let row = text.rows.get(line).ok_or(FilesError::LineTooLarge {
            given: line,
            max: text.rows.len(),
        })?;
        Ok(row.start_column
            + text.text[row.bytes.start..byte.min(row.bytes.end)]
                .chars()
                .count())
    }

    fn line_range(&'a self, id: &'a str, line: usize) -> Result<Range<usize>, FilesError> {
        let text = self.texts.get(id).ok_or(FilesError::FileMissing)?;
        text.rows
            .get(line)
            .map(|row| row.bytes.clone())
            .ok_or(FilesError::LineTooLarge {
                given: line,
                max: text.rows.len(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use tola_build::codes::typst::COMPILE;
    use tola_build::diagnostic::{Diagnostic, Help, Severity, SourceLine, SourcePosition};

    /// One location whose line the renderer can show without reading a file.
    fn snippet(line: usize, text: &str) -> Location {
        Location {
            path: "site.typ".to_owned(),
            line: Some(line),
            column: Some(1),
            range: None,
            source_lines: vec![SourceLine {
                line,
                start_column: 0,
                start_character: 0,
                ends_line: true,
                text: text.to_owned(),
                highlight: Some((0, 1)),
            }],
        }
    }

    /// A location's window is the source its producer read, so an edited file on disk does not
    /// decide what a running session shows.
    #[test]
    fn excerpts_outrank_disk_text() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("site.typ"), "stale disk line\n").unwrap();
        let sources = SourceFiles::new(
            directory.path().to_path_buf(),
            tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
        );
        let mut diagnostic = Diagnostic::at_location(
            COMPILE,
            Severity::Error,
            snippet(2, "#document(\"fresh/index.html\")[B]"),
            "path `fresh/index.html` occurs multiple times in the bundle",
        );
        diagnostic.help.push(Help {
            message: "path is already in use here".into(),
            location: Some(snippet(1, "#document(\"fresh/index.html\")[A]")),
        });

        let rendered = render(&diagnostic, Some(&sources), false);
        // Both labels are drawn, so both locations reached one text: the primary's window alone
        // would leave the line above it empty.
        assert!(rendered.contains("[A]"), "{rendered}");
        assert!(rendered.contains("[B]"), "{rendered}");
        assert!(!rendered.contains("stale disk line"), "{rendered}");
    }

    #[test]
    fn range_past_window_labels_first_line() {
        let mut location = snippet(3, "let wide = \"text");
        location.range = Some(SourceRange {
            start: SourcePosition {
                line: 2,
                character: 11,
            },
            end: SourcePosition {
                line: 20,
                character: 0,
            },
        });
        let rendered = render(
            &Diagnostic::at_location(COMPILE, Severity::Error, location, "unterminated string"),
            None,
            false,
        );

        assert!(rendered.contains("let wide = \"text"), "{rendered}");
        assert!(rendered.contains('^'), "{rendered}");
        assert!(
            rendered.contains("source excerpts are shortened"),
            "{rendered}"
        );
    }

    #[test]
    fn missing_excerpt_keeps_location() {
        let mut location = snippet(1, "#let value = ");
        location.source_lines[0].ends_line = false;
        location.column = Some(40);
        let end = location.source_lines[0].text.len();
        location.source_lines[0].highlight = Some((end, end));
        location.range = Some(SourceRange {
            start: SourcePosition {
                line: 0,
                character: 39,
            },
            end: SourcePosition {
                line: 0,
                character: 43,
            },
        });
        let rendered = render(
            &Diagnostic::at_location(COMPILE, Severity::Error, location, "unknown name"),
            None,
            false,
        );
        assert!(rendered.contains("site.typ:1:39"), "{rendered}");
        assert!(!rendered.contains('^'), "{rendered}");
        assert!(
            !rendered.contains("source excerpts are shortened"),
            "{rendered}"
        );
    }
    #[test]
    fn disjoint_windows_keep_endpoints() {
        for newline in [false, true] {
            let mut location = snippet(2_000_000, "fail(\"");
            location.column = Some(5002);
            location.range = Some(SourceRange {
                start: SourcePosition {
                    line: 1_999_999,
                    character: 5002,
                },
                end: if newline {
                    SourcePosition {
                        line: 2_000_000,
                        character: 0,
                    }
                } else {
                    SourcePosition {
                        line: 1_999_999,
                        character: 10013,
                    }
                },
            });
            location.source_lines[0].start_column = 5001;
            location.source_lines[0].start_character = 5002;
            location.source_lines[0].ends_line = false;
            location.source_lines.push(SourceLine {
                line: 2_000_000,
                start_column: 10010,
                start_character: 10011,
                ends_line: true,
                text: "\")".into(),
                highlight: Some((0, 2)),
            });
            let diagnostic = Diagnostic::at_location(COMPILE, Severity::Error, location, "failure");
            let escaped = DiagnosticText::of(&diagnostic);
            let files = RenderedFiles::of(&escaped, None);
            let primary = escaped.primary.as_ref().unwrap();
            let compact = files.compact_span(primary).unwrap();
            assert!(compact.starts_with("fail("));
            assert!(compact.ends_with("\")"));
            assert!(files.source("site.typ").unwrap().len() <= 9);
            let rendered = render(&diagnostic, None, false);
            assert!(rendered.contains("site.typ:2000000:5001"), "{rendered}");
            assert!(
                rendered.contains("source excerpts are shortened"),
                "{rendered}"
            );
        }
    }

    #[test]
    fn overlapping_windows_share_source() {
        let mut location = snippet(8, "abcdef");
        location.column = Some(11);
        location.source_lines[0].start_column = 10;
        location.source_lines[0].start_character = 10;
        location.source_lines[0].ends_line = false;
        let mut diagnostic = Diagnostic::at_location(COMPILE, Severity::Error, location, "failure");
        let mut help = snippet(8, "cdefgh");
        help.column = Some(13);
        help.source_lines[0].start_column = 12;
        help.source_lines[0].start_character = 12;
        diagnostic.help.push(Help {
            message: "inspect here".into(),
            location: Some(help),
        });
        let escaped = DiagnosticText::of(&diagnostic);
        let files = RenderedFiles::of(&escaped, None);
        assert_eq!(files.source("site.typ").unwrap(), "abcdefgh");
        let rendered = render(&diagnostic, None, false);
        assert!(rendered.contains("site.typ:8:10"), "{rendered}");
        assert!(rendered.contains("inspect here"), "{rendered}");
    }
    #[test]
    fn trailing_newline_keeps_closing_span() {
        let mut location = snippet(1, "call[");
        location.range = Some(SourceRange {
            start: SourcePosition {
                line: 0,
                character: 0,
            },
            end: SourcePosition {
                line: 2,
                character: 0,
            },
        });
        location.source_lines.push(SourceLine {
            line: 2,
            start_column: 0,
            start_character: 0,
            ends_line: true,
            text: "body]".into(),
            highlight: Some((0, 5)),
        });
        let mut diagnostic = Diagnostic::at_location(COMPILE, Severity::Error, location, "failure");
        diagnostic.help.push(Help {
            message: "inspect here".into(),
            location: Some(snippet(3, "next[]")),
        });
        let escaped = DiagnosticText::of(&diagnostic);
        let files = RenderedFiles::of(&escaped, None);
        let compact = files
            .compact_span(escaped.primary.as_ref().unwrap())
            .unwrap();
        assert!(compact.starts_with("call["));
        assert!(compact.ends_with(']'));
        let rendered = render(&diagnostic, None, false);
        assert!(rendered.contains("body]"), "{rendered}");
    }
    #[test]
    fn clipped_lines_keep_missing_suffix() {
        for end_character in [0, 1] {
            let mut location = snippet(1, "call[");
            location.source_lines[0].ends_line = false;
            location.range = Some(SourceRange {
                start: SourcePosition {
                    line: 0,
                    character: 0,
                },
                end: SourcePosition {
                    line: 1,
                    character: end_character,
                },
            });
            let mut diagnostic =
                Diagnostic::at_location(COMPILE, Severity::Error, location, "failure");
            diagnostic.help.push(Help {
                message: "inspect here".into(),
                location: Some(snippet(2, "]")),
            });
            let escaped = DiagnosticText::of(&diagnostic);
            let files = RenderedFiles::of(&escaped, None);
            let primary = escaped.primary.as_ref().unwrap();
            assert!(files.is_span_shortened(primary));
            if end_character == 0 {
                assert!(files.compact_span(primary).unwrap().ends_with('…'));
            }
            let rendered = render(&diagnostic, None, false);
            assert!(
                rendered.contains("source excerpts are shortened"),
                "{rendered}"
            );
        }
    }
}
