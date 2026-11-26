//! Typst span resolution with bounded source extraction.

use std::ops::Range;

use typst::World;
use typst::WorldExt;
use typst::syntax::{DiagSpan, FileId, Lines, Source, Span, VirtualRoot};

use super::NativeDiagnostic;
use super::location::LocatedSource;
use super::message::{
    Diagnostic, Hint, LocationFailure, SourceContextLimit, SourceLine, SourceLocation,
    SourcePosition, SourceRange, SourceTruncation, Trace, TraceKind,
};

struct SpanLocation {
    path: String,
    start_line: usize,
    start_col: usize,
    range: SourceRange,
    lines: Vec<SourceLine>,
    truncation: SourceTruncation,
}

impl SpanLocation {
    fn resolve<W: World>(
        world: &W,
        span: DiagSpan,
        limit: SourceContextLimit,
    ) -> Result<Self, LocationFailure> {
        let fid = span.id().ok_or(LocationFailure::DetachedSpan)?;
        let src = world
            .source(fid)
            .map_err(|_| LocationFailure::SourceUnavailable)?;
        let bytes = world.range(span).ok_or(LocationFailure::RangeUnavailable)?;
        let text = src.text();
        validate_range(text, &bytes)?;

        let lines = src.lines();
        let raw_start = lines
            .byte_to_line(bytes.start)
            .ok_or(LocationFailure::OutOfBounds)?;
        let raw_end = lines
            .byte_to_line(bytes.end)
            .ok_or(LocationFailure::OutOfBounds)?;
        let start_col = lines
            .byte_to_column(bytes.start)
            .ok_or(LocationFailure::InvalidUtf8Boundary)?;
        let range = SourceRange {
            start: utf16_pos(text, lines, bytes.start, raw_start)?,
            end: utf16_pos(text, lines, bytes.end, raw_end)?,
        };
        let path = id_path(fid);
        let (excerpt, truncation) = if limit.max_lines == 0 {
            (Vec::new(), SourceTruncation::default())
        } else {
            extract_lines(&src, &bytes, raw_start, raw_end, range.end.character, limit)
        };

        Ok(Self {
            path,
            start_line: raw_start + 1,
            start_col,
            range,
            lines: excerpt,
            truncation,
        })
    }

    fn into_location(self) -> SourceLocation {
        SourceLocation {
            path: Some(self.path),
            line: Some(self.start_line),
            column: Some(self.start_col + 1),
            range: Some(self.range),
            source_lines: self.lines,
            location_failure: None,
            truncation: self.truncation,
        }
    }
}

fn id_path(fid: FileId) -> String {
    match fid.root() {
        VirtualRoot::Package(spec) => format!("{spec}/{}", fid.vpath().get_without_slash()),
        VirtualRoot::Project => fid.vpath().get_without_slash().to_string(),
    }
}

fn utf16_pos(
    text: &str,
    lines: &Lines<String>,
    byte: usize,
    line: usize,
) -> Result<SourcePosition, LocationFailure> {
    let start = lines
        .line_to_byte(line)
        .ok_or(LocationFailure::OutOfBounds)?;
    Ok(SourcePosition {
        line,
        character: text[start..byte].encode_utf16().count(),
    })
}

fn extract_lines(
    src: &Source,
    bytes: &Range<usize>,
    raw_start: usize,
    raw_end: usize,
    end_char: usize,
    limit: SourceContextLimit,
) -> (Vec<SourceLine>, SourceTruncation) {
    let source_lines = src.lines();
    // An exclusive endpoint at the next line's start does not retain that empty line.
    let last_line = raw_end.saturating_sub(usize::from(raw_end > raw_start && end_char == 0));
    let covered_lines = last_line - raw_start + 1;
    let retained_lines = limit.max_lines.min(covered_lines);
    let head = retained_lines.div_ceil(2);
    let tail = retained_lines / 2;
    let selected = (raw_start..raw_start + head).chain(last_line + 1 - tail..last_line + 1);
    let mut lines = Vec::with_capacity(retained_lines + 1);
    let mut retained_bytes = 0;

    for line in selected {
        let line_text = source_line_text(src, line);
        let line_start = source_lines
            .line_to_byte(line)
            .expect("indexed source line");
        let highlight = if raw_start == raw_end {
            bytes.start - line_start..bytes.end - line_start
        } else {
            let start = if line == raw_start {
                bytes.start - line_start
            } else {
                0
            };
            let end = if line == raw_end {
                bytes.end - line_start
            } else {
                line_text.len()
            };
            start.min(line_text.len())..end.min(line_text.len())
        };
        let windows = source_windows(
            line_text,
            highlight.clone(),
            limit.max_line_bytes,
            raw_start == last_line,
            line == last_line && line != raw_start,
        );
        for window in windows {
            let retained = &line_text[window.clone()];
            let clipped_start = highlight.start.clamp(window.start, window.end);
            let clipped_end = highlight.end.clamp(window.start, window.end);
            lines.push(SourceLine {
                line_num: line + 1,
                start_column: line_text[..window.start].chars().count(),
                start_character: line_text[..window.start].encode_utf16().count(),
                ends_line: window.end == line_text.len(),
                text: retained.to_owned(),
                highlight: Some((
                    line_text[window.start..clipped_start].chars().count(),
                    line_text[window.start..clipped_end].chars().count(),
                )),
            });
            retained_bytes += retained.len();
        }
    }

    let covered_bytes: usize = (raw_start..=last_line)
        .map(|line| source_line_text(src, line).len())
        .sum();
    let truncation = SourceTruncation {
        omitted_lines: covered_lines - retained_lines,
        omitted_bytes: covered_bytes - retained_bytes,
    };
    (lines, truncation)
}

fn resolve_span<W: World>(world: &W, span: DiagSpan, limit: SourceContextLimit) -> SourceLocation {
    match SpanLocation::resolve(world, span, limit) {
        Ok(loc) => loc.into_location(),
        Err(err) => SourceLocation::unresolved(err),
    }
}

fn source_line_text(source: &Source, line: usize) -> &str {
    let range = source
        .lines()
        .line_to_range(line)
        .expect("indexed source line");
    source.text()[range].trim_end_matches(typst::syntax::is_newline)
}

fn source_windows(
    text: &str,
    highlight: std::ops::Range<usize>,
    max_bytes: usize,
    same_line: bool,
    keep_end: bool,
) -> Vec<std::ops::Range<usize>> {
    if text.len() <= max_bytes {
        return std::iter::once(0..text.len()).collect();
    }
    if same_line && highlight.len() > max_bytes && max_bytes > 1 {
        let head_bytes = max_bytes.div_ceil(2);
        let tail_bytes = max_bytes / 2;
        return vec![
            source_window(text, highlight.start..highlight.start, head_bytes, false),
            source_window(text, highlight.end..highlight.end, tail_bytes, true),
        ];
    }
    vec![source_window(text, highlight, max_bytes, keep_end)]
}

fn source_window(
    text: &str,
    highlight: std::ops::Range<usize>,
    max_bytes: usize,
    keep_end: bool,
) -> std::ops::Range<usize> {
    let focus = if keep_end {
        highlight.end
    } else {
        highlight.start
    };
    let context = if highlight.len() <= max_bytes {
        (max_bytes - highlight.len()) / 3
    } else {
        0
    };
    let mut start = if keep_end {
        focus.saturating_sub(max_bytes)
    } else {
        focus.saturating_sub(context)
    };
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + max_bytes).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    start..end
}

fn validate_range(text: &str, range: &std::ops::Range<usize>) -> Result<(), LocationFailure> {
    if range.start > range.end {
        return Err(LocationFailure::ReversedRange);
    }
    if range.end > text.len() {
        return Err(LocationFailure::OutOfBounds);
    }
    if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
        return Err(LocationFailure::InvalidUtf8Boundary);
    }
    Ok(())
}

/// Resolve a diagnostic to structured info for custom rendering.
pub fn resolve_diagnostic<W: World>(world: &W, diag: &NativeDiagnostic) -> Diagnostic {
    resolve_diagnostic_with_options(world, diag, SourceContextLimit::default())
}

/// Resolve a diagnostic with explicit source context limits.
pub fn resolve_diagnostic_with_options<W: World>(
    world: &W,
    native: &NativeDiagnostic,
    limit: SourceContextLimit,
) -> Diagnostic {
    let diag = native.source();
    let location = resolve_span(world, native.primary_span(), limit);

    let hints = diag
        .hints
        .iter()
        .map(|hint| Hint {
            message: hint.v.to_string(),
            location: resolve_span(world, hint.span, limit),
        })
        .collect();

    let traces = native
        .retained_traces()
        .map(|t| {
            use typst::diag::Tracepoint;

            let kind = match &t.v {
                Tracepoint::Call(name) => TraceKind::Call(name.as_ref().map(ToString::to_string)),
                Tracepoint::Show(name) => TraceKind::Show(name.to_string()),
                Tracepoint::Import(name) => TraceKind::Import(name.to_string()),
                Tracepoint::Include(name) => TraceKind::Include(name.to_string()),
            };
            Trace {
                kind,
                message: t.v.to_string(),
                location: resolve_span(world, t.span.into(), limit),
            }
        })
        .collect();

    Diagnostic {
        origin: native.origin().clone(),
        severity: diag.severity,
        message: diag.message.to_string(),
        location,
        hints,
        traces,
        imported_by: Vec::new(),
        package_failure: None,
    }
}

/// Resolve one source span to the file and position it names.
///
/// A consumer that holds a span instead of a diagnostic — an exported element that
/// references another file, for example — uses this to name where its source wrote it.
/// Returns `None` when the span has no source file or names one this world cannot read.
/// A span inside an installed package resolves to the package's own identity, exactly like
/// the location of a resolved diagnostic.
pub fn resolve_source<W: World>(world: &W, span: Span) -> Option<LocatedSource> {
    let loc = SpanLocation::resolve(world, span.into(), SourceContextLimit::POSITION_ONLY).ok()?;
    Some(LocatedSource {
        path: loc.path,
        line: Some(loc.start_line),
        column: Some(loc.start_col + 1),
        range: Some(loc.range),
    })
}

/// Resolve a span with bounded excerpts from the compilation's immutable source.
///
/// Exported references and payloads retain these excerpts with their diagnostics, so later
/// terminal rendering does not read a different revision from disk. Returns `None` when the
/// span is detached or its source cannot be resolved.
pub fn resolve_source_location<W: World>(world: &W, span: Span) -> Option<SourceLocation> {
    SpanLocation::resolve(world, span.into(), SourceContextLimit::default())
        .ok()
        .map(SpanLocation::into_location)
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::diag::SourceDiagnostic;

    fn find_text(
        node: &typst::syntax::SyntaxNode,
        text: &str,
    ) -> Option<typst::syntax::SyntaxNode> {
        if node.leaf_text() == text {
            return Some(node.clone());
        }
        node.children().find_map(|child| find_text(child, text))
    }

    fn world(text: &str) -> (tempfile::TempDir, crate::TypstWorld) {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.typ");
        std::fs::write(&main, text).unwrap();
        let world = crate::TypstWorld::builder(&main, dir.path())
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        (dir, world)
    }

    #[test]
    fn resolve_source_position() {
        let text = "#let unused = 1\n#link(\"/nowhere.html\")[Read more]\n";
        let (_dir, world) = world(text);
        let src = world.source(world.main()).unwrap();
        let node = find_text(src.root(), "Read more").expect("the link body is a text node");
        let resolved = resolve_source(&world, node.span()).unwrap();

        let offset = text.find("Read more").unwrap();
        let line_start = text.find('\n').unwrap() + 1;
        assert_eq!(resolved.path, "main.typ");
        assert_eq!(resolved.line, Some(2));
        assert_eq!(resolved.column, Some(offset - line_start + 1));
        assert_eq!(
            resolved.range.unwrap().start,
            SourcePosition {
                line: 1,
                character: offset - line_start
            }
        );
        assert!(resolve_source(&world, Span::detached()).is_none());
    }

    #[test]
    fn resolve_diag_truncates_excerpt() {
        let text = format!("{}🦊alpha\r\nβ🦊z", " ".repeat(5000));
        let (_dir, world) = world(&text);
        let src = world.source(world.main()).unwrap();
        let span = DiagSpan::from_range(world.main(), 5004..text.len());
        let mut diag = SourceDiagnostic::error(span, "multiline failure").with_tracepoint(
            typst::diag::Tracepoint::Call(Some("f".into())),
            src.root().span(),
        );
        diag.hints.push(typst::syntax::Spanned::new(
            "inspect this span".into(),
            span,
        ));
        let diag = NativeDiagnostic::from(diag);
        let resolved = resolve_diagnostic_with_options(
            &world,
            &diag,
            SourceContextLimit {
                max_lines: 1,
                max_line_bytes: 16,
            },
        );
        let expected = SourceRange {
            start: SourcePosition {
                line: 0,
                character: 5002,
            },
            end: SourcePosition {
                line: 1,
                character: 4,
            },
        };
        assert_eq!(resolved.location.range, Some(expected));
        assert_eq!(resolved.hints[0].location.range, Some(expected));
        assert_eq!(
            resolved.traces[0].location.range,
            Some(SourceRange {
                start: SourcePosition {
                    line: 0,
                    character: 0
                },
                end: expected.end,
            })
        );
        assert_eq!(resolved.location.source_lines[0].text, "alpha");
        assert_eq!(resolved.location.source_lines[0].start_column, 5001);
        assert_eq!(resolved.location.source_lines[0].start_character, 5002);
        assert_eq!(resolved.location.source_lines.len(), 1);
        assert_eq!(resolved.location.truncation.omitted_lines, 1);
        assert_eq!(
            resolved.location.truncation.omitted_bytes,
            5000 + "🦊alphaβ🦊z".len() - "alpha".len()
        );
    }

    #[test]
    fn resolve_diag_newlines() {
        let (_dir, world) = world("a\rb\r\nc\u{85}d\u{2028}é\u{2029}f\nz");
        let src = world.source(world.main()).unwrap();
        let last = src
            .root()
            .children()
            .find(|node| node.leaf_text() == "z")
            .unwrap();
        let diag = NativeDiagnostic::from(SourceDiagnostic::error(last.span(), "last line"));
        let resolved = resolve_diagnostic(&world, &diag);
        assert_eq!(
            (resolved.location.line, resolved.location.column),
            (Some(7), Some(1))
        );
        assert_eq!(resolved.location.source_lines[0].text, "z");

        let diag =
            NativeDiagnostic::from(SourceDiagnostic::error(src.root().span(), "whole source"));
        let resolved = resolve_diagnostic_with_options(
            &world,
            &diag,
            SourceContextLimit {
                max_lines: 2,
                max_line_bytes: 1,
            },
        );
        assert_eq!(
            resolved
                .location
                .source_lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(
            resolved.location.truncation,
            SourceTruncation {
                omitted_lines: 5,
                omitted_bytes: 6,
            }
        );
    }

    #[test]
    fn validate_range_rejects_invalid() {
        let text = "aéz";
        assert_eq!(
            validate_range(text, &std::ops::Range { start: 3, end: 2 }),
            Err(LocationFailure::ReversedRange)
        );
        assert_eq!(
            validate_range(text, &(0..99)),
            Err(LocationFailure::OutOfBounds)
        );
        assert_eq!(
            validate_range(text, &(2..3)),
            Err(LocationFailure::InvalidUtf8Boundary)
        );
        assert_eq!(validate_range(text, &(1..3)), Ok(()));
    }
}
