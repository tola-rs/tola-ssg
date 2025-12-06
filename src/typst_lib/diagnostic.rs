//! Diagnostic formatting for Typst compilation errors and warnings.
//!
//! This module provides human-readable formatting for `SourceDiagnostic` similar
//! to the official `typst-cli` output, including:
//!
//! - File path, line number, and column information
//! - Source code snippets with error location markers
//! - Colored output with theme support (error=red, warning=yellow, help=cyan)
//! - Hints and trace information
//!
//! # Architecture
//!
//! The module is organized into several layers:
//! - **Theme**: Color styling for different diagnostic severities
//! - **Gutter**: Box-drawing characters for source display
//! - **SpanLocation**: Resolved source location information
//! - **SnippetWriter**: Handles formatted output generation
//!
//! # Example Output
//!
//! ```text
//! error: `invalid:meta.typ` is not a valid package namespace
//!   ┌─ content/index.typ:1:8
//!   │
//! 1 │ #import "@invalid:meta.typ" as meta
//!   │         ^^^^^^^^^^^^^^^^
//! ```

use std::fmt::Write;

use colored::{ColoredString, Colorize};
use typst::diag::{Severity, SourceDiagnostic};
use typst::syntax::Span;
use typst::World;

// ============================================================================
// Gutter Characters
// ============================================================================

/// Box-drawing characters for source code display.
mod gutter {
    pub const HEADER: &str = "┌─";
    pub const BAR: &str = "│";
    pub const SPAN_START: &str = "╭";
    pub const SPAN_END: &str = "╰";
    pub const DASH: &str = "─";
    pub const MARKER: &str = "^";
}

// ============================================================================
// Color Theme
// ============================================================================

/// Color theme for diagnostic output.
///
/// Provides consistent coloring for different diagnostic severities:
/// - Error: Red
/// - Warning: Yellow
/// - Help: Cyan
#[derive(Clone, Copy)]
struct DiagnosticTheme {
    colorize: fn(&str) -> ColoredString,
}

impl DiagnosticTheme {
    const ERROR: Self = Self {
        colorize: |s| s.red(),
    };
    const WARNING: Self = Self {
        colorize: |s| s.yellow(),
    };
    const HELP: Self = Self {
        colorize: |s| s.cyan(),
    };

    /// Apply theme color to any text element.
    #[inline]
    fn paint(&self, text: &str) -> ColoredString {
        (self.colorize)(text)
    }
}

// ============================================================================
// Span Location
// ============================================================================

/// Resolved source location information for a diagnostic span.
///
/// Contains all information needed to display a source code snippet
/// with proper highlighting.
struct SpanLocation {
    /// File path (relative to project root)
    path: String,
    /// Starting line number (1-indexed)
    start_line: usize,
    /// Starting column (1-indexed)
    start_col: usize,
    /// Source lines covered by the span
    lines: Vec<String>,
    /// Column where highlighting starts in first line (1-indexed)
    highlight_start_col: usize,
    /// Column where highlighting ends in last line (1-indexed, exclusive)
    highlight_end_col: usize,
}

impl SpanLocation {
    /// Resolve a span to its source location.
    fn from_span<W: World>(world: &W, span: Span) -> Option<Self> {
        let id = span.id()?;
        let source = world.source(id).ok()?;
        let range = source.range(span)?;
        let text = source.text();

        // Calculate line boundaries
        let start_line_start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let end_line_end = text[range.end..]
            .find('\n')
            .map_or(text.len(), |i| range.end + i);
        let end_line_start = text[..range.end].rfind('\n').map_or(0, |i| i + 1);

        // Calculate positions
        // Column numbers are 0-indexed to match typst-cli output
        let start_line = text[..range.start].matches('\n').count() + 1;
        let start_col = text[start_line_start..range.start].chars().count();
        let end_col = text[end_line_start..range.end].chars().count();

        // Extract source lines
        let lines = text[start_line_start..end_line_end]
            .lines()
            .map(String::from)
            .collect();

        // Build path
        let path = id.vpath().as_rootless_path().to_string_lossy().into_owned();

        Some(Self {
            path,
            start_line,
            start_col,
            lines,
            highlight_start_col: start_col,
            highlight_end_col: end_col,
        })
    }

    /// Check if this span covers multiple lines.
    #[inline]
    fn is_multiline(&self) -> bool {
        self.lines.len() > 1
    }

    /// Get the last line number covered by this span.
    #[inline]
    fn end_line(&self) -> usize {
        self.start_line + self.lines.len() - 1
    }

    /// Calculate the width needed to display line numbers.
    #[inline]
    fn line_num_width(&self) -> usize {
        self.end_line().to_string().len().max(1)
    }
}

// ============================================================================
// Snippet Writer
// ============================================================================

/// Helper for writing formatted source snippets.
///
/// Encapsulates the logic for formatting source code with proper
/// alignment, gutter characters, and highlighting.
struct SnippetWriter<'a> {
    output: &'a mut String,
    theme: &'a DiagnosticTheme,
    line_num_width: usize,
}

impl<'a> SnippetWriter<'a> {
    fn new(output: &'a mut String, theme: &'a DiagnosticTheme, line_num_width: usize) -> Self {
        Self {
            output,
            theme,
            line_num_width,
        }
    }

    /// Write the location header: "  ┌─ path:line:col"
    fn write_header(&mut self, path: &str, line: usize, col: usize) {
        let _ = writeln!(
            self.output,
            "{:>width$} {} {}:{}:{}",
            "",
            self.theme.paint(gutter::HEADER),
            path,
            line,
            col,
            width = self.line_num_width
        );
    }

    /// Write an empty gutter line: "  │"
    fn write_empty_gutter(&mut self) {
        let _ = writeln!(
            self.output,
            "{:>width$} {}",
            "",
            self.theme.paint(gutter::BAR),
            width = self.line_num_width
        );
    }

    /// Write a source line with optional box character and highlighting.
    fn write_source_line(
        &mut self,
        line_num: usize,
        line_text: &str,
        box_char: Option<&str>,
        highlight_range: Option<(usize, usize)>,
    ) {
        let line_num_str = format!("{:>width$}", line_num, width = self.line_num_width);

        let formatted_line = match (box_char, highlight_range) {
            (Some(bc), Some((start, end))) => {
                let (before, highlighted, after) = self.split_line(line_text, start, end);
                format!(
                    "{} {} {} {}{}{}",
                    self.theme.paint(&line_num_str),
                    self.theme.paint(gutter::BAR),
                    self.theme.paint(bc),
                    before,
                    self.theme.paint(&highlighted),
                    after
                )
            }
            (None, Some((start, end))) => {
                let (before, highlighted, after) = self.split_line(line_text, start, end);
                format!(
                    "{} {} {}{}{}",
                    self.theme.paint(&line_num_str),
                    self.theme.paint(gutter::BAR),
                    before,
                    self.theme.paint(&highlighted),
                    after
                )
            }
            _ => {
                format!(
                    "{} {} {}",
                    self.theme.paint(&line_num_str),
                    self.theme.paint(gutter::BAR),
                    line_text
                )
            }
        };

        let _ = writeln!(self.output, "{}", formatted_line);
    }

    /// Write marker line for single-line spans: "  │   ^^^^"
    fn write_single_line_marker(&mut self, start_col: usize, span_len: usize) {
        let spaces = " ".repeat(start_col);
        let markers = gutter::MARKER.repeat(span_len.max(1));
        let _ = writeln!(
            self.output,
            "{:>width$} {} {}{}",
            "",
            self.theme.paint(gutter::BAR),
            spaces,
            self.theme.paint(&markers),
            width = self.line_num_width
        );
    }

    /// Write marker line for multi-line spans: "  │ ╰────^"
    fn write_multiline_end_marker(&mut self, end_col: usize) {
        let dashes = gutter::DASH.repeat(end_col);
        let _ = writeln!(
            self.output,
            "{:>width$} {} {}{}{}",
            "",
            self.theme.paint(gutter::BAR),
            self.theme.paint(gutter::SPAN_END),
            self.theme.paint(&dashes),
            self.theme.paint(gutter::MARKER),
            width = self.line_num_width
        );
    }

    /// Split a line into (before, highlighted, after) based on column range.
    /// Both start_col and end_col are 0-indexed.
    fn split_line(&self, line: &str, start_col: usize, end_col: usize) -> (String, String, String) {
        let chars: Vec<char> = line.chars().collect();
        let start_idx = start_col.min(chars.len());
        let end_idx = end_col.min(chars.len());

        let before: String = chars[..start_idx].iter().collect();
        let highlighted: String = chars[start_idx..end_idx].iter().collect();
        let after: String = chars[end_idx..].iter().collect();

        (before, highlighted, after)
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Format compilation diagnostics into a human-readable string.
///
/// Errors are displayed first (higher priority), followed by warnings.
pub fn format_diagnostics<W: World>(world: &W, diagnostics: &[SourceDiagnostic]) -> String {
    let mut output = String::new();

    // Partition and sort: errors first, then warnings
    let (errors, warnings): (Vec<_>, Vec<_>) = diagnostics
        .iter()
        .partition(|d| d.severity == Severity::Error);

    for diag in errors.iter().chain(warnings.iter()) {
        format_diagnostic(&mut output, world, diag);
    }

    output
}

/// Count errors and warnings in a diagnostic list.
#[allow(dead_code)]
pub fn count_diagnostics(diagnostics: &[SourceDiagnostic]) -> (usize, usize) {
    diagnostics.iter().fold((0, 0), |(errors, warnings), d| {
        match d.severity {
            Severity::Error => (errors + 1, warnings),
            Severity::Warning => (errors, warnings + 1),
        }
    })
}

/// Check if there are any errors in the diagnostics.
pub fn has_errors(diagnostics: &[SourceDiagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}

/// Filter out known HTML export development warnings.
///
/// Typst's HTML export is experimental and always produces a warning.
/// This function filters out that warning to reduce noise in error output.
pub fn filter_html_warnings(diagnostics: &[SourceDiagnostic]) -> Vec<SourceDiagnostic> {
    diagnostics
        .iter()
        .filter(|d| {
            // Keep all errors
            if d.severity == Severity::Error {
                return true;
            }
            // Filter out HTML export warning
            !d.message.contains("html export is under active development")
        })
        .cloned()
        .collect()
}

// ============================================================================
// Diagnostic Formatting (Internal)
// ============================================================================

/// Format a single diagnostic with its source snippet.
fn format_diagnostic<W: World>(output: &mut String, world: &W, diag: &SourceDiagnostic) {
    let (label, theme) = match diag.severity {
        Severity::Error => ("error", DiagnosticTheme::ERROR),
        Severity::Warning => ("warning", DiagnosticTheme::WARNING),
    };

    // Header: "error: message"
    let _ = writeln!(output, "{}: {}", theme.paint(label), diag.message);

    // Source snippet
    if let Some(location) = SpanLocation::from_span(world, diag.span) {
        write_snippet(output, &location, &theme);
    }

    // Trace information (call stack)
    for trace in &diag.trace {
        write_trace(output, world, &trace.v, trace.span);
    }

    // Hints
    for hint in &diag.hints {
        let _ = writeln!(
            output,
            "  {} hint: {}",
            DiagnosticTheme::HELP.paint("="),
            hint
        );
    }
}

/// Write a source code snippet with highlighting.
fn write_snippet(output: &mut String, location: &SpanLocation, theme: &DiagnosticTheme) {
    let mut writer = SnippetWriter::new(output, theme, location.line_num_width());

    writer.write_header(&location.path, location.start_line, location.start_col);
    writer.write_empty_gutter();

    if location.is_multiline() {
        write_multiline_snippet(&mut writer, location);
    } else {
        write_singleline_snippet(&mut writer, location);
    }
}

/// Write a single-line source snippet.
fn write_singleline_snippet(writer: &mut SnippetWriter, location: &SpanLocation) {
    let line_text = location.lines.first().map(|s| s.as_str()).unwrap_or("");
    let span_len = location
        .highlight_end_col
        .saturating_sub(location.highlight_start_col)
        .max(1);

    writer.write_source_line(
        location.start_line,
        line_text,
        None,
        Some((location.highlight_start_col, location.highlight_end_col)),
    );
    writer.write_single_line_marker(location.highlight_start_col, span_len);
}

/// Write a multi-line source snippet with box drawing.
fn write_multiline_snippet(writer: &mut SnippetWriter, location: &SpanLocation) {
    for (i, line_text) in location.lines.iter().enumerate() {
        let line_num = location.start_line + i;
        let is_first = i == 0;
        let line_len = line_text.chars().count();

        let (box_char, highlight_range) = if is_first {
            (
                gutter::SPAN_START,
                (location.highlight_start_col, line_len + 1),
            )
        } else {
            (gutter::BAR, (1, line_len + 1))
        };

        writer.write_source_line(line_num, line_text, Some(box_char), Some(highlight_range));
    }

    writer.write_multiline_end_marker(location.highlight_end_col);
}

/// Write trace information with help theme.
fn write_trace<W: World>(
    output: &mut String,
    world: &W,
    tracepoint: &typst::diag::Tracepoint,
    span: Span,
) {
    use typst::diag::Tracepoint;

    let message = match tracepoint {
        Tracepoint::Call(Some(name)) => {
            format!("error occurred in this call of function `{}`", name)
        }
        Tracepoint::Call(None) => "error occurred in this function call".into(),
        Tracepoint::Show(name) => format!("error occurred in this show rule for `{}`", name),
        Tracepoint::Import => "error occurred in this import".into(),
    };

    let _ = writeln!(
        output,
        "{}: {}",
        DiagnosticTheme::HELP.paint("help"),
        message
    );

    if let Some(location) = SpanLocation::from_span(world, span) {
        write_snippet(output, &location, &DiagnosticTheme::HELP);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn disable_colors() {
        colored::control::set_override(false);
    }

    #[test]
    fn test_format_source_snippet_single_line() {
        disable_colors();

        let location = SpanLocation {
            path: "content/index.typ".to_string(),
            start_line: 1,
            start_col: 8, // 0-indexed
            lines: vec!["#import \"@tola:meta.typ\" as meta".to_string()],
            highlight_start_col: 8, // 0-indexed
            highlight_end_col: 24,  // 0-indexed, exclusive
        };

        let mut output = String::new();
        write_snippet(&mut output, &location, &DiagnosticTheme::ERROR);

        assert!(output.contains("content/index.typ:1:8"));
        assert!(output.contains("#import \"@tola:meta.typ\" as meta"));
        assert!(output.contains("^^^^^^^^^^^^^^^^"));
    }

    #[test]
    fn test_format_source_snippet_multiline() {
        disable_colors();

        let location = SpanLocation {
            path: "templates/normal.typ".to_string(),
            start_line: 32,
            start_col: 2, // 0-indexed
            lines: vec![
                "  meta((".to_string(),
                "    title: title,".to_string(),
                "    date: date,".to_string(),
                "  ))".to_string(),
            ],
            highlight_start_col: 2, // 0-indexed
            highlight_end_col: 4,   // 0-indexed, exclusive
        };

        let mut output = String::new();
        write_snippet(&mut output, &location, &DiagnosticTheme::ERROR);

        assert!(output.contains("templates/normal.typ:32:2"));
        assert!(output.contains("meta(("));
        assert!(output.contains("title: title"));
        assert!(output.contains("╭") || output.contains("╰"));
    }

    #[test]
    fn test_format_source_snippet_line_number_width() {
        disable_colors();

        let location = SpanLocation {
            path: "test.typ".to_string(),
            start_line: 123,
            start_col: 4, // 0-indexed
            lines: vec!["    some code".to_string()],
            highlight_start_col: 4, // 0-indexed
            highlight_end_col: 8,   // 0-indexed, exclusive
        };

        let mut output = String::new();
        write_snippet(&mut output, &location, &DiagnosticTheme::ERROR);

        assert!(output.contains("123 │"));
        assert!(output.contains("^^^^"));
    }

    #[test]
    fn test_count_diagnostics() {
        let diags = vec![
            SourceDiagnostic::error(Span::detached(), "error 1"),
            SourceDiagnostic::error(Span::detached(), "error 2"),
            SourceDiagnostic::warning(Span::detached(), "warning 1"),
            SourceDiagnostic::warning(Span::detached(), "warning 2"),
        ];

        let (errors, warnings) = count_diagnostics(&diags);
        assert_eq!(errors, 2);
        assert_eq!(warnings, 2);
    }

    #[test]
    fn test_has_errors() {
        let warnings_only = vec![
            SourceDiagnostic::warning(Span::detached(), "warning 1"),
            SourceDiagnostic::warning(Span::detached(), "warning 2"),
        ];
        assert!(!has_errors(&warnings_only));

        let with_errors = vec![
            SourceDiagnostic::warning(Span::detached(), "warning 1"),
            SourceDiagnostic::error(Span::detached(), "error 1"),
        ];
        assert!(has_errors(&with_errors));

        let empty: Vec<SourceDiagnostic> = vec![];
        assert!(!has_errors(&empty));
    }

    #[test]
    fn test_split_line_helper() {
        // Test the split logic without SnippetWriter
        // Note: start_col and end_col are 0-indexed
        fn split_line(line: &str, start_col: usize, end_col: usize) -> (String, String, String) {
            let chars: Vec<char> = line.chars().collect();
            let start_idx = start_col.min(chars.len());
            let end_idx = end_col.min(chars.len());

            let before: String = chars[..start_idx].iter().collect();
            let highlighted: String = chars[start_idx..end_idx].iter().collect();
            let after: String = chars[end_idx..].iter().collect();

            (before, highlighted, after)
        }

        // Test normal case: "hello world", cols 6-11 -> "hello " + "world" + ""
        let (before, highlighted, after) = split_line("hello world", 6, 11);
        assert_eq!(before, "hello ");
        assert_eq!(highlighted, "world");
        assert_eq!(after, "");

        // Test start at beginning: "abc", cols 0-1 -> "" + "a" + "bc"
        let (before, highlighted, after) = split_line("abc", 0, 1);
        assert_eq!(before, "");
        assert_eq!(highlighted, "a");
        assert_eq!(after, "bc");

        // Test full line: "test", cols 0-4 -> "" + "test" + ""
        let (before, highlighted, after) = split_line("test", 0, 4);
        assert_eq!(before, "");
        assert_eq!(highlighted, "test");
        assert_eq!(after, "");

        // Test with Unicode: "你好世界", cols 0-2 -> "" + "你好" + "世界"
        let (before, highlighted, after) = split_line("你好世界", 0, 2);
        assert_eq!(before, "");
        assert_eq!(highlighted, "你好");
        assert_eq!(after, "世界");
    }

    #[test]
    fn test_span_location_methods() {
        let location = SpanLocation {
            path: "test.typ".to_string(),
            start_line: 10,
            start_col: 0, // 0-indexed
            lines: vec!["line1".into(), "line2".into(), "line3".into()],
            highlight_start_col: 0,
            highlight_end_col: 5,
        };

        assert!(location.is_multiline());
        assert_eq!(location.end_line(), 12);
        assert_eq!(location.line_num_width(), 2);

        let single_line = SpanLocation {
            path: "test.typ".to_string(),
            start_line: 5,
            start_col: 0, // 0-indexed
            lines: vec!["single".into()],
            highlight_start_col: 0,
            highlight_end_col: 6,
        };

        assert!(!single_line.is_multiline());
        assert_eq!(single_line.end_line(), 5);
        assert_eq!(single_line.line_num_width(), 1);
    }
}
