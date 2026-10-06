//! Diagnostic rendering through `codespan-reporting`, the renderer the Typst CLI uses.
//!
//! Tola contributes resolved data: the path of each location as the site author reads it, the
//! exact position of primary and help labels, and the notes. Call traces follow the primary
//! diagnostic as compact source spans, matching the Typst CLI.

mod source_text;

use std::io::Write;

use codespan_reporting::diagnostic::{Diagnostic as Renderable, Label};
use codespan_reporting::term::termcolor::{Buffer, Color, ColorSpec, WriteColor};
use codespan_reporting::term::{self, Config};
use tola_build::diagnostic::{Diagnostic, Location, Severity};

use super::source::SourceFiles;
use super::text;
use source_text::RenderedFiles;

/// Render a site diagnostic with its compact call trace.
pub(crate) fn render(
    diagnostic: &Diagnostic,
    sources: Option<&SourceFiles>,
    use_color: bool,
) -> String {
    let escaped = DiagnosticText::of(diagnostic);
    let files = RenderedFiles::of(&escaped, sources);
    let config = Config {
        // The Typst CLI renders with two-column tabs.
        tab_width: 2,
        ..Default::default()
    };
    let rendered = renderable(diagnostic, &escaped, &files);
    // A buffer collects the layout, so one diagnostic renders to one string the caller places.
    // The no-color buffer strips styling rather than disabling `codespan-reporting`, which would
    // also cost the layout that belongs to it.
    let mut buffer = if use_color {
        Buffer::ansi()
    } else {
        Buffer::no_color()
    };
    // Labels address retained source rows, and the in-memory Buffer cannot fail a write.
    term::emit(&mut buffer, &config, &files, &rendered)
        .expect("diagnostic labels address retained source rows");
    for frame in &escaped.trace {
        write!(buffer, "  {}", text::single_line(&frame.message))
            .expect("writing to a Buffer succeeds");
        if frame.location.is_some() {
            write!(buffer, " at ").expect("writing to a Buffer succeeds");
            buffer
                .set_color(ColorSpec::new().set_underline(true))
                .expect("styling a Buffer succeeds");
            write!(buffer, "{}", frame.spelled()).expect("writing to a Buffer succeeds");
            buffer.reset().expect("styling a Buffer succeeds");
        }
        writeln!(buffer).expect("writing to a Buffer succeeds");
        if let Some(source) = files.compact_span(frame)
            && !source.is_empty()
        {
            write!(buffer, "    ").expect("writing to a Buffer succeeds");
            buffer
                .set_color(ColorSpec::new().set_fg(Some(Color::Ansi256(248))))
                .expect("styling a Buffer succeeds");
            write!(buffer, "{source}").expect("writing to a Buffer succeeds");
            buffer.reset().expect("styling a Buffer succeeds");
            writeln!(buffer).expect("writing to a Buffer succeeds");
        }
    }
    let rendered = String::from_utf8_lossy(buffer.as_slice());
    rendered.trim_end_matches('\n').to_owned()
}

/// The codespan diagnostic Tola's record maps onto.
fn renderable<'a>(
    diagnostic: &Diagnostic,
    escaped: &'a DiagnosticText<'_>,
    files: &RenderedFiles<'a>,
) -> Renderable<&'a str> {
    let mut rendered = match diagnostic.severity {
        Severity::Error => Renderable::error(),
        Severity::Warning => Renderable::warning(),
    }
    .with_message(escaped.message.as_str())
    .with_labels(labels(escaped, files))
    .with_notes(notes(escaped, files));
    rendered.code = Some(diagnostic.code.to_string());
    rendered
}

/// Every note the diagnostic has, including text that has no label to sit under.
fn notes(escaped: &DiagnosticText<'_>, files: &RenderedFiles<'_>) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(primary) = &escaped.primary
        && files.range(primary).is_none()
    {
        notes.push(format!("location: {}", primary.spelled()));
    }
    notes.extend(escaped.notes.iter().cloned());
    for located in &escaped.help {
        if files.range(located).is_none() {
            notes.push(located.note());
        }
    }
    if escaped
        .primary
        .iter()
        .chain(&escaped.help)
        .any(|located| files.range(located).is_some() && files.is_span_shortened(located))
    {
        notes.push("source excerpts are shortened".to_owned());
    }
    notes
}

/// A diagnostic record's text as a terminal may draw it.
///
/// `codespan-reporting` writes messages, notes, and paths verbatim, so this is where a diagnostic
/// stops being able to control the terminal it is printed on. Every string is escaped for display
/// and the record is left as the producer wrote it.
struct DiagnosticText<'a> {
    message: String,
    notes: Vec<String>,
    /// The diagnostic's own location, which it has whenever it names a source.
    primary: Option<LocationText<'a>>,
    /// One location text per help entry the terminal draws, in the order the record holds them.
    help: Vec<LocationText<'a>>,
    /// One location text per trace frame the terminal draws, in the order the record holds them.
    trace: Vec<LocationText<'a>>,
}

impl<'a> DiagnosticText<'a> {
    fn of(diagnostic: &'a Diagnostic) -> Self {
        Self {
            message: text::multiline(diagnostic.display_message()),
            notes: diagnostic.distinct_notes().map(text::multiline).collect(),
            primary: diagnostic.location.as_ref().map(LocationText::primary),
            help: diagnostic
                .distinct_help()
                .map(|help| LocationText::of("help", &help.message, help.location.as_ref()))
                .collect(),
            trace: diagnostic
                .trace
                .iter()
                .map(|frame| LocationText::of("trace", &frame.message, frame.location.as_ref()))
                .collect(),
        }
    }

    /// Every location text this diagnostic has: its own first, then one per help and trace.
    fn locations(&self) -> impl Iterator<Item = &LocationText<'a>> {
        self.primary.iter().chain(&self.help).chain(&self.trace)
    }
}

/// One location a diagnostic points at, with the text the terminal reads on it.
struct LocationText<'a> {
    /// The word it reads under when no snippet can show it: `location`, `help`, or `trace`.
    kind: &'static str,
    /// Where it points, when the record named a file.
    location: Option<&'a Location>,
    /// The display path of the file it points at, escaped.
    path: Option<String>,
    /// The text the entry has: help and trace have one, the diagnostic's own location does not.
    message: String,
}

impl<'a> LocationText<'a> {
    /// The text of the diagnostic's own location.
    fn primary(location: &'a Location) -> Self {
        Self {
            kind: "location",
            location: Some(location),
            path: Some(text::single_line(&location.path)),
            message: String::new(),
        }
    }

    /// The text of a help or trace entry, which may point nowhere at all.
    fn of(kind: &'static str, message: &str, location: Option<&'a Location>) -> Self {
        Self {
            kind,
            location,
            path: location.map(|location| text::single_line(&location.path)),
            message: text::multiline(message),
        }
    }

    /// The path the renderer knows this location by.
    fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The entry as a note reads it.
    fn note(&self) -> String {
        match self.location {
            Some(_) => format!("{}: {}: {}", self.kind, self.spelled(), self.message),
            None => format!("{}: {}", self.kind, self.message),
        }
    }

    /// The location spelled the way a reader can search for it.
    ///
    /// One the record left without a file reads as its own text instead.
    fn spelled(&self) -> String {
        let (Some(path), Some(location)) = (self.path(), self.location) else {
            return self.note();
        };
        match (location.line, location.column) {
            (Some(line), Some(column)) => format!("{path}:{line}:{}", column.saturating_sub(1)),
            (Some(line), None) => format!("{path}:{line}"),
            _ => path.to_owned(),
        }
    }
}

/// Located help belongs beside the code it corrects; call traces have their own compact layout.
fn labels<'a>(escaped: &'a DiagnosticText<'_>, files: &RenderedFiles<'a>) -> Vec<Label<&'a str>> {
    let mut labels = Vec::new();
    if let Some(primary) = &escaped.primary
        && let (Some(path), Some(range)) = (primary.path(), files.range(primary))
    {
        labels.push(Label::primary(path, range));
    }
    for located in &escaped.help {
        if let (Some(path), Some(range)) = (located.path(), files.range(located)) {
            labels.push(Label::secondary(path, range).with_message(located.message.clone()));
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::codes::hook::COMMAND;
    use tola_build::codes::typst::COMPILE;
    use tola_build::diagnostic::{Help, SourceLine, SourcePosition, SourceRange, TraceFrame};
    use unicode_width::UnicodeWidthStr;

    /// One location whose line the renderer can show without reading a file.
    fn snippet(line: usize, text: &str) -> Location {
        Location {
            path: "site.typ".to_owned(),
            line: Some(line),
            column: Some(1),
            range: None,
            source_lines: vec![SourceLine {
                line,
                text: text.to_owned(),
                start_column: 0,
                start_character: 0,
                ends_line: true,
                highlight: Some((0, 1)),
            }],
        }
    }

    /// A located help states what is wrong at the location it points at, so its text belongs on the
    /// code a reader compares, not only in the headline.
    #[test]
    fn located_help_keeps_its_text() {
        let mut diagnostic = Diagnostic::at_location(
            COMPILE,
            Severity::Error,
            snippet(2, "#document(\"same/index.html\")[B]"),
            "path `same/index.html` occurs multiple times in the bundle",
        );
        diagnostic.help.push(Help {
            message: "path is already in use here".into(),
            location: Some(snippet(1, "#document(\"same/index.html\")[A]")),
        });

        let rendered = render(&diagnostic, None, false);
        assert!(
            rendered.contains("path is already in use here"),
            "{rendered}"
        );
    }

    /// A help whose file the renderer cannot read still reaches the reader as a note.
    #[test]
    fn help_without_source_text_renders_as_note() {
        let mut diagnostic = Diagnostic::at_path(
            COMPILE,
            Severity::Error,
            "site.typ",
            "path `same/index.html` occurs multiple times in the bundle",
        );
        diagnostic.help.push(Help {
            message: "path is already in use here".into(),
            location: Some(Location {
                path: "other.typ".to_owned(),
                line: Some(1),
                column: Some(1),
                range: None,
                source_lines: Vec::new(),
            }),
        });

        let rendered = render(&diagnostic, None, false);
        assert!(rendered.contains("other.typ:1:0"), "{rendered}");
        assert!(
            rendered.contains("path is already in use here"),
            "{rendered}"
        );
    }

    /// Escaping a source line for display never moves a caret: what replaces a control has the
    /// byte length of the character it hides, and a wide character keeps its width.
    #[test]
    fn escapes_keep_every_caret_on_its_character() {
        for line in ["let x = \"a\u{1b}z\";", "\"界\u{7}z\""] {
            let highlighted = line.find('z').expect("the excerpt marks `z`");
            let mut location = snippet(1, line);
            location.source_lines[0].highlight = Some((highlighted, highlighted + 1));
            let rendered = render(
                &Diagnostic::at_location(COMPILE, Severity::Error, location, "unexpected control"),
                None,
                false,
            );

            assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
            assert!(!rendered.contains('\u{7}'), "{rendered:?}");
            let rows = rendered.lines().collect::<Vec<_>>();
            let source_index = rows
                .iter()
                .position(|row| row.contains('z'))
                .expect("the source line is drawn");
            let source = rows[source_index];
            assert!(
                source.contains(line.replace(['\u{1b}', '\u{7}'], "?").as_str()),
                "{rendered}"
            );
            let caret = rows
                .get(source_index + 1)
                .and_then(|row| row.find('^'))
                .expect("a caret follows the source line");
            assert_eq!(
                rows[source_index + 1][..caret].width(),
                source[..source.find('z').expect("the line keeps `z`")].width(),
                "{rendered}"
            );
        }
    }

    /// A failing hook's output is text Tola did not write: the drawn diagnostic drops the escape
    /// sequences that colored it and shows the carriage return it has instead of obeying it.
    #[test]
    fn hook_note_controls_stay_visible() {
        let diagnostic = Diagnostic::new(
            COMMAND,
            Severity::Error,
            "the `styles` hook in `build.hooks.before-build` failed: the command exited with \
             status 3",
        )
        .with_note(
            "stderr:\nplain-err fail\n\u{1b}[32mansi-err\u{1b}[0m\ncr-err cccc\rdddd\neeee\n",
        );

        let rendered = render(&diagnostic, None, false);
        assert!(
            !rendered.contains(['\u{1b}', '\r', '\u{7}']),
            "{rendered:?}"
        );
        assert!(rendered.contains("plain-err fail"), "{rendered:?}");
        assert!(rendered.contains("ansi-err"), "{rendered:?}");
        assert!(rendered.contains("cr-err cccc\\rdddd"), "{rendered:?}");
    }

    /// Only Tola's own styling reaches the terminal as an escape; the hook's text adds none.
    #[test]
    fn foreign_text_adds_no_raw_escape() {
        let diagnostic = |note: &str| {
            Diagnostic::new(COMMAND, Severity::Error, "the `styles` hook failed")
                .with_note(note.to_owned())
        };
        let foreign = render(
            &diagnostic("stderr:\n\u{1b}[32mansi-err\u{1b}[0m\n"),
            None,
            true,
        );
        let plain = render(&diagnostic("stderr:\nansi-err\n"), None, true);

        assert!(
            plain.contains('\u{1b}'),
            "a styled render styles with escapes: {plain:?}"
        );
        assert_eq!(foreign, plain, "{foreign:?}");
    }

    #[test]
    fn multiline_trace_stays_compact() {
        let mut diagnostic = Diagnostic::at_location(
            COMPILE,
            Severity::Error,
            snippet(1, "#panic(\"missing home page\")"),
            "missing home page",
        );
        let mut location = snippet(4, "#page-shell[");
        location.column = Some(2);
        location.range = Some(SourceRange {
            start: SourcePosition {
                line: 3,
                character: 1,
            },
            end: SourcePosition {
                line: 14,
                character: 1,
            },
        });
        location.source_lines.extend((5..15).map(|line| SourceLine {
            line,
            text: "  page body must stay out of the trace".to_owned(),
            start_column: 0,
            start_character: 0,
            ends_line: true,
            highlight: None,
        }));
        location.source_lines.push(SourceLine {
            line: 15,
            text: "]".to_owned(),
            start_column: 0,
            start_character: 0,
            ends_line: true,
            highlight: Some((0, 1)),
        });
        let frame = TraceFrame {
            message: "while calling `page-shell`".to_owned(),
            location: Some(location),
        };
        diagnostic.trace.extend([frame.clone(), frame]);

        let rendered = render(&diagnostic, None, false);
        assert!(rendered.contains("page-shell[…]"), "{rendered}");
        assert_eq!(
            rendered.matches("while calling `page-shell`").count(),
            2,
            "{rendered}"
        );
        assert_eq!(rendered.matches("site.typ:4:1").count(), 2, "{rendered}");
        assert!(!rendered.contains("page body"), "{rendered}");
        assert!(
            !rendered.contains("source excerpts are shortened"),
            "{rendered}"
        );
    }

    #[test]
    fn compact_trace_marks_missing_endpoint() {
        let mut diagnostic = Diagnostic::new(COMPILE, Severity::Error, "missing page");
        let mut location = snippet(3, "#🦊 + 页面[界👩‍💻e\u{301}\u{1b}");
        location.column = Some(6);
        location.range = Some(SourceRange {
            start: SourcePosition {
                line: 2,
                character: 6,
            },
            end: SourcePosition {
                line: 20,
                character: 1,
            },
        });
        diagnostic.trace.push(TraceFrame {
            message: "while calling `页面`".to_owned(),
            location: Some(location),
        });

        let rendered = render(&diagnostic, None, false);
        assert!(rendered.contains("页面[界👩‍💻e\u{301}?…"), "{rendered}");
        assert!(!rendered.contains("?…]"), "{rendered}");
        assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
        assert!(
            !rendered.contains("source excerpts are shortened"),
            "{rendered}"
        );
    }

    #[test]
    fn repeated_advice_keeps_distinct_locations() {
        let mut diagnostic = Diagnostic::new(COMPILE, Severity::Error, "missing page")
            .with_note("the home page needs content")
            .with_note("the home page needs content");
        let help = Help {
            message: "add a page here".to_owned(),
            location: Some(snippet(2, "#page(\"index.html\")[Home]")),
        };
        diagnostic.help.extend([help.clone(), help]);
        diagnostic.help.push(Help {
            message: "add a page here".to_owned(),
            location: Some(snippet(4, "#page(\"about.html\")[About]")),
        });

        let rendered = render(&diagnostic, None, false);
        assert_eq!(
            rendered.matches("the home page needs content").count(),
            1,
            "{rendered}"
        );
        assert_eq!(rendered.matches("add a page here").count(), 2, "{rendered}");
        assert!(rendered.contains("[Home]"), "{rendered}");
        assert!(rendered.contains("[About]"), "{rendered}");
    }

    #[test]
    fn package_locations_keep_referents() {
        let mut diagnostic = Diagnostic::at_location(
            COMPILE,
            Severity::Error,
            snippet(2, "#tola-meta((title: \"A\", authros: \"x\"))"),
            "`$.authros`: is not a declared field",
        );
        let mut location = snippet(93, "report-source-issues(issues)");
        location.path = "@tola/source:0.0.0/lib.typ".to_owned();
        location.source_lines[0].highlight = Some((0, location.source_lines[0].text.len()));
        diagnostic.trace.push(tola_build::diagnostic::TraceFrame {
            message: "while calling `tola-report-source-issues`".into(),
            location: Some(location),
        });

        let mut location = snippet(12, "  let parsed = try-parse(metadata, schema)");
        location.path = "@tola/schema:0.0.0/lib.typ".to_owned();
        diagnostic.help.push(Help {
            message: "check the metadata declaration".into(),
            location: Some(location),
        });

        let rendered = render(&diagnostic, None, false);
        assert!(
            rendered.contains("@tola/source:0.0.0/lib.typ:93:0"),
            "{rendered}"
        );
        assert!(
            rendered.contains("while calling `tola-report-source-issues`"),
            "{rendered}"
        );
        assert!(
            rendered.contains("report-source-issues(issues)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("@tola/schema:0.0.0/lib.typ:12:0"),
            "{rendered}"
        );
        assert!(
            rendered.contains("try-parse(metadata, schema)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("check the metadata declaration"),
            "{rendered}"
        );
    }

    #[test]
    fn trace_styling_follows_color_choice() {
        let mut diagnostic = Diagnostic::new(COMPILE, Severity::Error, "missing page");
        diagnostic.trace.push(TraceFrame {
            message: "while calling `page`".into(),
            location: Some(snippet(1, "#page[]")),
        });
        let styled = render(&diagnostic, None, true);
        assert!(styled.contains("\u{1b}[4m"), "{styled:?}");
        assert!(styled.contains("\u{1b}[38;5;248m"), "{styled:?}");
        let plain = render(&diagnostic, None, false);
        assert!(!plain.contains('\u{1b}'), "{plain:?}");
        assert!(plain.contains("site.typ:1:0"), "{plain}");
    }

    #[test]
    fn anonymous_trace_keeps_location() {
        let mut diagnostic = Diagnostic::at_location(
            COMPILE,
            Severity::Error,
            snippet(2, "#let value = absent"),
            "unknown variable: absent",
        );
        diagnostic.trace.push(tola_build::diagnostic::TraceFrame {
            message: "while calling function".into(),
            location: Some(snippet(1, "#select-pages(all-sources())")),
        });

        let rendered = render(&diagnostic, None, false);
        assert!(rendered.contains("while calling function"), "{rendered}");
        assert!(rendered.contains("site.typ:1:0"), "{rendered}");
    }

    #[test]
    fn primary_and_trace_share_columns() {
        let location = snippet(1, "#panic(\"missing page\")");
        let mut diagnostic =
            Diagnostic::at_location(COMPILE, Severity::Error, location.clone(), "missing page");
        diagnostic.trace.push(TraceFrame {
            message: "while calling function".to_owned(),
            location: Some(location),
        });
        let rendered = render(&diagnostic, None, false);
        assert_eq!(rendered.matches("site.typ:1:0").count(), 2, "{rendered}");
        assert_eq!(diagnostic.location.as_ref().unwrap().column, Some(1));
        assert_eq!(
            diagnostic.trace[0].location.as_ref().unwrap().column,
            Some(1)
        );
    }
}
