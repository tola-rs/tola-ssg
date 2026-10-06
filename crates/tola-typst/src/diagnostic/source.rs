//! Typst span resolution with bounded source extraction.

use super::NativeDiagnostic;
use typst::World;
use typst::WorldExt;
use typst::syntax::{DiagSpan, Source, Span};

use super::message::{
    Diagnostic, Hint, LocationFailure, SourceContextLimit, SourceLine, SourceLocation,
    SourcePosition, SourceRange, SourceTruncation, Trace, TraceKind,
};
use super::resolved::ResolvedSource;

/// Resolved span location and bounded source lines for highlighting.
struct SpanLocation {
    /// File path relative to the compilation root. A package file is named by the package the
    /// author imported plus its package-relative path, so the location never names a host path.
    path: String,
    /// Starting line number (1-indexed)
    start_line: usize,
    /// Zero-based Unicode scalar column of the span's start.
    start_col: usize,
    /// Exact range from the same immutable source that produced the diagnostic.
    range: SourceRange,
    /// Source lines retained for display.
    lines: Vec<SourceLine>,
    /// Truncation applied while extracting source context.
    truncation: SourceTruncation,
}

impl SpanLocation {
    /// Resolve a span while directly bounding retained source context.
    fn resolve<W: World>(
        world: &W,
        span: DiagSpan,
        limit: SourceContextLimit,
    ) -> Result<Self, LocationFailure> {
        let id = span.id().ok_or(LocationFailure::DetachedSpan)?;
        let source = world
            .source(id)
            .map_err(|_| LocationFailure::SourceUnavailable)?;
        let bytes = world.range(span).ok_or(LocationFailure::RangeUnavailable)?;
        let text = source.text();
        validate_range(text, &bytes)?;

        let source_lines = source.lines();
        let raw_start = source_lines
            .byte_to_line(bytes.start)
            .ok_or(LocationFailure::OutOfBounds)?;
        let raw_end = source_lines
            .byte_to_line(bytes.end)
            .ok_or(LocationFailure::OutOfBounds)?;
        let start_line = raw_start + 1;
        let start_col = source_lines
            .byte_to_column(bytes.start)
            .ok_or(LocationFailure::InvalidUtf8Boundary)?;
        let position = |byte, line| -> Result<SourcePosition, LocationFailure> {
            let start = source_lines
                .line_to_byte(line)
                .ok_or(LocationFailure::OutOfBounds)?;
            Ok(SourcePosition {
                line,
                character: text[start..byte].encode_utf16().count(),
            })
        };
        let range = SourceRange {
            start: position(bytes.start, raw_start)?,
            end: position(bytes.end, raw_end)?,
        };
        let path = match id.root() {
            typst::syntax::VirtualRoot::Package(spec) => {
                format!("{spec}/{}", id.vpath().get_without_slash())
            }
            typst::syntax::VirtualRoot::Project => id.vpath().get_without_slash().to_string(),
        };
        if limit.max_lines == 0 {
            return Ok(Self {
                path,
                start_line,
                start_col,
                range,
                lines: Vec::new(),
                truncation: SourceTruncation::default(),
            });
        }

        // An exclusive endpoint at the next line's start does not retain that empty line.
        let last_line =
            raw_end.saturating_sub(usize::from(raw_end > raw_start && range.end.character == 0));
        let covered_lines = last_line - raw_start + 1;
        let retained_lines = limit.max_lines.min(covered_lines);
        let head_lines = retained_lines.div_ceil(2);
        let tail_lines = retained_lines / 2;
        let selected =
            (raw_start..raw_start + head_lines).chain(last_line + 1 - tail_lines..last_line + 1);
        let mut lines = Vec::with_capacity(retained_lines + 1);
        let mut retained_bytes = 0;
        for line in selected {
            let line_text = source_line_text(&source, line);
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
        let covered_bytes = (raw_start..=last_line)
            .map(|line| source_line_text(&source, line).len())
            .sum::<usize>();
        let truncation = SourceTruncation {
            omitted_lines: covered_lines - retained_lines,
            omitted_bytes: covered_bytes - retained_bytes,
        };

        Ok(Self {
            path,
            start_line,
            start_col,
            range,
            lines,
            truncation,
        })
    }

    /// Project this resolved span into the location a diagnostic, hint, or trace has.
    ///
    /// Every resolved span keeps its path and position, including a span inside an installed
    /// package: that path is the package's own identity plus a package-relative file, so it names
    /// where the diagnostic came from without exposing a host path.
    fn location(&self) -> SourceLocation {
        SourceLocation {
            path: Some(self.path.clone()),
            line: Some(self.start_line),
            column: Some(self.start_col + 1),
            range: Some(self.range),
            source_lines: self.lines.clone(),
            location_failure: None,
            truncation: self.truncation,
        }
    }

    /// Project one resolution, or the failure that replaced it.
    fn resolved_location(resolution: &Result<Self, LocationFailure>) -> SourceLocation {
        match resolution {
            Ok(location) => location.location(),
            Err(failure) => SourceLocation::unresolved(*failure),
        }
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
    context_limit: SourceContextLimit,
) -> Diagnostic {
    let diag = native.source();
    let location =
        SpanLocation::resolved_location(&SpanLocation::resolve(world, diag.span, context_limit));

    let hints = diag
        .hints
        .iter()
        .map(|hint| Hint {
            message: hint.v.to_string(),
            location: SpanLocation::resolved_location(&SpanLocation::resolve(
                world,
                hint.span,
                context_limit,
            )),
        })
        .collect();

    let traces = diag
        .trace
        .iter()
        .map(|t| {
            use typst::diag::Tracepoint;

            let kind = match &t.v {
                Tracepoint::Call(name) => TraceKind::Call(name.as_ref().map(ToString::to_string)),
                Tracepoint::Show(name) => TraceKind::Show(name.to_string()),
                Tracepoint::Import(name) => TraceKind::Import(name.to_string()),
                Tracepoint::Include(name) => TraceKind::Include(name.to_string()),
            };
            let message = t.v.to_string();
            let resolved = SpanLocation::resolve(world, t.span.into(), context_limit);
            Trace {
                kind,
                message,
                location: SpanLocation::resolved_location(&resolved),
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
pub fn resolve_source<W: World>(world: &W, span: Span) -> Option<ResolvedSource> {
    let location =
        SpanLocation::resolve(world, span.into(), SourceContextLimit::POSITION_ONLY).ok()?;
    Some(ResolvedSource {
        path: location.path,
        line: Some(location.start_line),
        column: Some(location.start_col + 1),
        range: Some(location.range),
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
        .map(|location| location.location())
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
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.typ");
        std::fs::write(&main, text).unwrap();
        let world = crate::TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        (directory, world)
    }

    #[test]
    fn span_resolves_to_written_position() {
        let text = "#let unused = 1\n#link(\"/nowhere.html\")[Read more]\n";
        let (_directory, world) = world(text);
        let source = world.source(world.main()).unwrap();
        let node = find_text(source.root(), "Read more").expect("the link body is a text node");
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
    fn source_location_keeps_compiled_text() {
        let original = "#link(\"/missing.html\")[Read more]\n";
        let (directory, world) = world(original);
        let source = world.source(world.main()).unwrap();
        let span = find_text(source.root(), "Read more")
            .expect("the link body is a source text node")
            .span();
        std::fs::write(directory.path().join("main.typ"), "Changed on disk\n").unwrap();
        let location = resolve_source_location(&world, span).unwrap();
        assert_eq!(location.path.as_deref(), Some("main.typ"));
        assert_eq!(location.source_lines[0].text, original.trim_end());
        assert!(resolve_source_location(&world, Span::detached()).is_none());
    }

    #[test]
    fn native_message_stays_intact() {
        let (_directory, world) = world("#panic(\"this broke\")\n");
        let source = world.source(world.main()).unwrap();

        let panicked = NativeDiagnostic::from(SourceDiagnostic::error(
            source.root().span(),
            "panicked with: this broke",
        ));
        assert_eq!(
            resolve_diagnostic(&world, &panicked).message,
            "panicked with: this broke"
        );

        let ordinary = NativeDiagnostic::from(SourceDiagnostic::error(
            source.root().span(),
            "expected string, found content",
        ));
        assert_eq!(
            resolve_diagnostic(&world, &ordinary).message,
            "expected string, found content"
        );
    }

    #[cfg(feature = "legacy-serialization")]
    #[allow(deprecated)]
    #[test]
    fn ranges_survive_excerpt_truncation() {
        let text = format!("{}🦊alpha\r\nβ🦊z", " ".repeat(5000));
        let (_directory, world) = world(&text);
        let source = world.source(world.main()).unwrap();
        let span = DiagSpan::from_range(world.main(), 5004..text.len());
        let mut diagnostic = SourceDiagnostic::error(span, "multiline failure").with_tracepoint(
            typst::diag::Tracepoint::Call(Some("f".into())),
            source.root().span(),
        );
        diagnostic.hints.push(typst::syntax::Spanned::new(
            "inspect this span".into(),
            span,
        ));
        let diagnostic = NativeDiagnostic::from(diagnostic);
        let resolved = resolve_diagnostic_with_options(
            &world,
            &diagnostic,
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
        let serialized = serde_json::to_string(&resolved.to_resolved()).unwrap();
        let decoded: crate::ResolvedDiagnostic = serde_json::from_str(&serialized).unwrap();
        assert_eq!(decoded.source.unwrap().range, Some(expected));
        assert_eq!(
            decoded.hints[0].source.as_ref().unwrap().range,
            Some(expected)
        );
        assert_eq!(
            decoded.trace[0].source.as_ref().unwrap().range.unwrap().end,
            expected.end
        );
    }

    #[test]
    fn native_newlines_keep_locations() {
        let (_directory, world) = world("a\rb\r\nc\u{85}d\u{2028}é\u{2029}f\nz");
        let source = world.source(world.main()).unwrap();
        let last = source
            .root()
            .children()
            .find(|node| node.leaf_text() == "z")
            .unwrap();
        let diagnostic = NativeDiagnostic::from(SourceDiagnostic::error(last.span(), "last line"));
        let resolved = resolve_diagnostic(&world, &diagnostic);
        assert_eq!(
            (resolved.location.line, resolved.location.column),
            (Some(7), Some(1))
        );
        assert_eq!(resolved.location.source_lines[0].text, "z");

        let diagnostic = NativeDiagnostic::from(SourceDiagnostic::error(
            source.root().span(),
            "whole source",
        ));
        let resolved = resolve_diagnostic_with_options(
            &world,
            &diagnostic,
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
    fn excerpts_keep_character_boundaries() {
        let (_directory, world) = world("ééé\nsecond");
        let source = world.source(world.main()).unwrap();
        let location = SpanLocation::resolve(
            &world,
            source.root().span().into(),
            SourceContextLimit {
                max_lines: 1,
                max_line_bytes: 3,
            },
        )
        .unwrap();

        assert_eq!(location.lines.len(), 1);
        assert_eq!(location.lines[0].text, "é");
        assert_eq!(location.lines[0].highlight, Some((0, 1)));
        assert_eq!(location.truncation.omitted_lines, 1);
        assert_eq!(location.truncation.omitted_bytes, 10);
    }

    #[test]
    fn excerpt_keeps_span_edges() {
        let text = format!("call(\n{}end)", "body\n".repeat(40));
        let (_directory, world) = world(&text);
        let location = SpanLocation::resolve(
            &world,
            DiagSpan::from_range(world.main(), 0..text.len()),
            SourceContextLimit {
                max_lines: 4,
                max_line_bytes: 16,
            },
        )
        .unwrap();
        assert_eq!(
            location
                .lines
                .iter()
                .map(|line| line.line_num)
                .collect::<Vec<_>>(),
            vec![1, 2, 41, 42]
        );
        assert_eq!(location.lines.first().unwrap().text, "call(");
        assert_eq!(location.lines.last().unwrap().text, "end)");
        assert_eq!(location.truncation.omitted_lines, 38);
    }

    #[test]
    fn clipped_prefix_keeps_call_position() {
        let prefix = format!("{}🦊", "a".repeat(5000));
        let text = format!("{prefix}call(1)");
        let (_directory, world) = world(&text);
        let location = SpanLocation::resolve(
            &world,
            DiagSpan::from_range(world.main(), prefix.len()..text.len()),
            SourceContextLimit {
                max_lines: 1,
                max_line_bytes: 32,
            },
        )
        .unwrap();
        let line = &location.lines[0];
        let (start, end) = line.highlight.unwrap();
        assert_eq!(
            line.text
                .chars()
                .skip(start)
                .take(end - start)
                .collect::<String>(),
            "call(1)"
        );
        assert_eq!(location.start_col, 5001);
        assert_eq!(location.range.start.character, 5002);
        assert_eq!(line.start_column + start, 5001);
        assert_eq!(
            line.start_character
                + line
                    .text
                    .chars()
                    .take(start)
                    .map(char::len_utf16)
                    .sum::<usize>(),
            5002
        );
        assert!(line.text.len() <= 32);
    }

    #[test]
    fn wide_span_keeps_bounded_edges() {
        for newline in ["", "\n"] {
            let text = format!("call(\"{}\"){newline}", "a".repeat(5000));
            let (_directory, world) = world(&text);
            let location = SpanLocation::resolve(
                &world,
                DiagSpan::from_range(world.main(), 0..text.len()),
                SourceContextLimit {
                    max_lines: 1,
                    max_line_bytes: 32,
                },
            )
            .unwrap();
            assert_eq!(location.lines.len(), 2);
            assert!(location.lines[0].text.starts_with("call("));
            assert!(!location.lines[0].ends_line);
            assert!(location.lines[1].text.ends_with("\")"));
            assert!(location.lines[1].ends_line);
            let retained_bytes = location
                .lines
                .iter()
                .map(|line| line.text.len())
                .sum::<usize>();
            assert!(retained_bytes <= 32);
            assert_eq!(
                location.truncation.omitted_bytes + retained_bytes,
                text.len() - newline.len()
            );
            let expected_end = if newline.is_empty() {
                SourcePosition {
                    line: 0,
                    character: text.len(),
                }
            } else {
                SourcePosition {
                    line: 1,
                    character: 0,
                }
            };
            assert_eq!(location.range.end, expected_end);
            assert_eq!(
                location.lines[1].start_character + location.lines[1].text.encode_utf16().count(),
                text.len() - newline.len()
            );
        }
    }

    #[test]
    fn position_only_skips_excerpt() {
        let text = format!("{}\nlast", "x".repeat(96));
        let (_directory, world) = world(&text);
        let span = DiagSpan::from_range(world.main(), 97..text.len());

        let location =
            SpanLocation::resolve(&world, span, SourceContextLimit::POSITION_ONLY).unwrap();

        assert_eq!((location.start_line, location.start_col), (2, 0));
        assert!(location.lines.is_empty());
        assert_eq!(location.truncation, SourceTruncation::default());
    }

    #[test]
    fn resolution_keeps_diagnostic_structure() {
        use typst::diag::Tracepoint;

        let (_directory, world) = world("#let f() = 1\n#f()\n");
        let source = world.source(world.main()).unwrap();
        let span = source.root().span();
        let mut diagnostic = SourceDiagnostic::error(span, "failure").with_hint("detached");
        for tracepoint in [
            Tracepoint::Call(Some("f".into())),
            Tracepoint::Show("text".into()),
            Tracepoint::Import("module.typ".into()),
            Tracepoint::Include("chapter.typ".into()),
        ] {
            diagnostic = diagnostic.with_tracepoint(tracepoint, span);
        }

        let diagnostic = NativeDiagnostic::from(diagnostic);
        let info = resolve_diagnostic(&world, &diagnostic);
        assert_eq!(info.hints.len(), 1);
        assert_eq!(
            info.hints[0].location.location_failure,
            Some(LocationFailure::DetachedSpan)
        );
        assert!(matches!(info.traces[0].kind, TraceKind::Call(_)));
        assert!(matches!(info.traces[1].kind, TraceKind::Show(_)));
        assert!(matches!(info.traces[2].kind, TraceKind::Import(_)));
        assert!(matches!(info.traces[3].kind, TraceKind::Include(_)));
    }

    #[test]
    fn invalid_ranges_are_rejected() {
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
