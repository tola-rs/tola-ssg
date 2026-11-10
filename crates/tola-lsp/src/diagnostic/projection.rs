//! One compiler diagnostic as the editor reads it: severity, code, ranges, related text, and the
//! file text those ranges are projected through.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use lsp_types::{
    Diagnostic as EditorDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DiagnosticTag, Location as EditorLocation, NumberOrString, Position, Range, Uri,
};
use tola_diagnostic::{
    Diagnostic, DiagnosticSource, Location, Severity, SourcePosition, SourceRange,
};
use tola_typst_syntax::position::{Utf16Position, Utf16Range, byte_offset as source_byte_offset};
use tola_typst_syntax::typst_syntax::Lines;

use crate::position;
use crate::sources::OpenSources;

use super::refine::{refine_refused_source, refine_unresolved_import};

/// The record range one protocol range addresses.
pub(super) fn protocol_source_range(range: Range) -> SourceRange {
    SourceRange {
        start: SourcePosition {
            line: range.start.line as usize,
            character: range.start.character as usize,
        },
        end: SourcePosition {
            line: range.end.line as usize,
            character: range.end.character as usize,
        },
    }
}

/// The record range one compiler-model range addresses.
pub(crate) fn compiler_source_range(range: Utf16Range) -> SourceRange {
    let at = |position: Utf16Position| SourcePosition {
        line: position.line as usize,
        character: position.character as usize,
    };
    SourceRange {
        start: at(range.start),
        end: at(range.end),
    }
}

/// The site path one diagnostic location names, absent when it names a package file.
pub(super) fn location_path(root: &Path, location: &Location) -> Option<PathBuf> {
    if location.path.starts_with('@') {
        return None;
    }
    let path = Path::new(&location.path);
    Some(tola_input::normalize_existing_prefix(
        &if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        },
    ))
}

/// A validated report maps only to the exact text its host evidence describes.
fn evidence_path(
    root: &Path,
    location: &Location,
    source: Option<&DiagnosticSource>,
    texts: &mut DiagnosticTexts<'_>,
) -> Option<PathBuf> {
    match source {
        Some(source) => {
            let path = tola_input::normalize_existing_prefix(source.editor_path()?);
            let text = texts.text(&path)?;
            (text.digest == source.input_identity().1).then_some(path)
        }
        None => location_path(root, location),
    }
}

/// Where one diagnostic's advice goes: beside the location it names, or in the message.
struct Advice<'a, 'b> {
    root: &'a Path,
    client_root: &'a crate::uri::ClientRoot,
    fold_related: bool,
    projected: &'b mut EditorDiagnostic,
    texts: &'b mut DiagnosticTexts<'a>,
    folded: Vec<String>,
}

impl<'a, 'b> Advice<'a, 'b> {
    fn push(
        &mut self,
        location: Option<&Location>,
        source: Option<&DiagnosticSource>,
        label: &str,
        message: &str,
    ) -> Result<()> {
        let located = if self.fold_related { None } else { location };
        if let Some(location) = located
            && let Some(path) = evidence_path(self.root, location, source, self.texts)
        {
            add_related(
                self.projected,
                self.client_root,
                &path,
                location,
                message,
                self.texts,
            )?;
            return Ok(());
        }
        self.folded
            .push(advice_line(self.root, location, label, message));
        Ok(())
    }
}

/// One diagnostic as the editor reads it, with `texts` supplying each range's file text.
pub(super) fn editor_diagnostic<'a>(
    mut diagnostic: Diagnostic,
    root: &'a Path,
    entry: &Path,
    client_root: &'a crate::uri::ClientRoot,
    fold_related: bool,
    texts: &mut DiagnosticTexts<'a>,
) -> Result<Option<(PathBuf, EditorDiagnostic)>> {
    use std::fmt::Write as _;

    let public_report = diagnostic.producer.is_some();
    if !public_report {
        refine_refused_source(&mut diagnostic, root);
        refine_unresolved_import(&mut diagnostic);
    }

    let primary = diagnostic
        .location
        .as_ref()
        .filter(|location| diagnostic.source.is_some() || !location.path.starts_with('@'))
        .or_else(|| {
            if public_report {
                return None;
            }
            diagnostic
                .trace
                .iter()
                .filter_map(|frame| frame.location.as_ref())
                .find(|location| !location.path.starts_with('@'))
        });
    let located = primary
        .and_then(|location| evidence_path(root, location, diagnostic.source.as_ref(), texts));
    if public_report
        && (located.is_none() || primary.is_none_or(|location| location.range.is_none()))
    {
        return Ok(None);
    }
    let range = diagnostic_range(primary, located.as_deref(), texts);
    let path = located.unwrap_or_else(|| entry.to_path_buf());
    let mut projected = EditorDiagnostic::new(
        range,
        Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Info => DiagnosticSeverity::INFORMATION,
            Severity::Hint => DiagnosticSeverity::HINT,
        }),
        match &diagnostic.producer {
            Some(producer) => producer.code.clone().map(NumberOrString::String),
            None => Some(NumberOrString::String(diagnostic.code.to_string())),
        },
        Some(
            diagnostic
                .producer
                .as_ref()
                .map_or("tola", |producer| {
                    producer.name.as_deref().unwrap_or("unknown")
                })
                .into(),
        ),
        diagnostic.display_message().to_owned(),
        None,
        None,
    );
    // The cause the diagnostic classified, which a client echoes back with the report it shows.
    if let Some(cause) = &diagnostic.cause {
        projected.data = serde_json::to_value(cause).ok();
    }
    // Code no spelling reads is dead weight, and the editor greys it out rather than counting it as
    // a mistake the author must fix.
    if diagnostic.code == tola_diagnostic::codes::check::UNUSED_IMPORT
        || diagnostic.code == crate::codes::editor::UNUSED_BINDING
        || diagnostic.code == crate::codes::editor::DEAD_STORE
    {
        projected.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
    }
    let mut advice = Advice {
        root,
        client_root,
        fold_related,
        projected: &mut projected,
        texts,
        folded: Vec::new(),
    };
    for help in diagnostic.distinct_help() {
        advice.push(help.location.as_ref(), None, "help: ", &help.message)?;
    }
    for related in &diagnostic.related {
        let location = related
            .location
            .as_ref()
            .filter(|location| location.range.is_some());
        advice.push(
            location,
            related.source.as_ref(),
            "related: ",
            &related.message,
        )?;
    }
    // A source metadata issue names a field, not a site call: the editor does not surface the
    // parse pipeline that reached it.
    if diagnostic.code != tola_diagnostic::codes::source::METADATA {
        for frame in &diagnostic.trace {
            advice.push(frame.location.as_ref(), None, "", &frame.message)?;
        }
    }
    let Advice { folded, .. } = advice;
    // The label lets an editor reader separate notes, help, and trace text from the message.
    for note in diagnostic.distinct_notes() {
        write!(&mut projected.message, "\nnote: {note}").expect("String formatting is infallible");
    }
    for line in folded {
        write!(&mut projected.message, "\n{line}").expect("String formatting is infallible");
    }
    Ok(Some((path, projected)))
}

fn spelled_location(root: &Path, location: &Location) -> String {
    use std::fmt::Write as _;

    let mut spelled = if location.path.starts_with('@') {
        location.path.clone()
    } else {
        tola_diagnostic::display_path(Path::new(&location.path), root)
    };
    if let Some(line) = location.line {
        write!(spelled, ":{line}").expect("String formatting is infallible");
        if let Some(column) = location.column {
            write!(spelled, ":{column}").expect("String formatting is infallible");
        }
    }
    spelled
}

/// The message line one help, related note, or trace frame reads as.
///
/// `label` separates it from the diagnostic's own message; an unlocated piece of advice carries
/// the label alone.
fn advice_line(root: &Path, location: Option<&Location>, label: &str, message: &str) -> String {
    match location {
        Some(location) => format!("{}: {label}{message}", spelled_location(root, location)),
        None => format!("{label}{message}"),
    }
}

/// One located help or trace that points into the site, as related information.
fn add_related(
    diagnostic: &mut EditorDiagnostic,
    client_root: &crate::uri::ClientRoot,
    path: &Path,
    location: &Location,
    message: &str,
    texts: &mut DiagnosticTexts<'_>,
) -> Result<()> {
    diagnostic
        .related_information
        .get_or_insert_with(Vec::new)
        .push(DiagnosticRelatedInformation {
            location: EditorLocation::new(
                client_root.address(path)?,
                diagnostic_range(Some(location), Some(path), texts),
            ),
            message: message.to_owned(),
        });
    Ok(())
}

/// The text of each file a diagnostic range is projected through, read once per publish.
///
/// An open document is the text the check compiled; a file no editor holds is read from disk, which
/// is the text the check read.
pub(super) struct DiagnosticTexts<'a> {
    open: &'a OpenSources,
    texts: BTreeMap<PathBuf, Option<ProjectedText>>,
}

/// One file's text as the projection reads it.
///
/// The compiler's line model can only be taken where it agrees with the protocol's, and where its
/// breaks first disagree is a property of the text: finding it once here is what keeps projecting
/// a file's every diagnostic from rescanning a prefix of the file per range.
struct ProjectedText {
    digest: tola_typst::ContentDigest,
    lines: Lines<String>,
    first_break: Option<usize>,
}

impl DiagnosticTexts<'_> {
    pub(super) fn new(open: &OpenSources) -> DiagnosticTexts<'_> {
        DiagnosticTexts {
            open,
            texts: BTreeMap::new(),
        }
    }

    /// The text of one file, absent when no text reaches it.
    fn text(&mut self, path: &Path) -> Option<&ProjectedText> {
        let open = self.open;
        self.texts
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                open_text(open, path)
                    .or_else(|| std::fs::read_to_string(path).ok())
                    .map(|text| ProjectedText {
                        digest: tola_typst::ContentDigest::of(text.as_bytes()),
                        first_break: crate::position::first_break(&text),
                        lines: Lines::new(text),
                    })
            })
            .as_ref()
    }
}

/// The text the editor holds for one file, absent when it holds none.
fn open_text(open: &OpenSources, path: &Path) -> Option<String> {
    let uri: Uri = open.versions_for_path(path).next()?.0.parse().ok()?;
    Some(open.source(&uri)?.text().to_owned())
}

/// The text of one compiler line of `lines`, without its ending.
fn line_text(lines: &Lines<String>, line: usize) -> Option<String> {
    let range = lines.line_to_range(line)?;
    Some(
        lines
            .text()
            .get(range)?
            .trim_end_matches(tola_typst_syntax::typst_syntax::is_newline)
            .to_owned(),
    )
}

/// The protocol positions of one compiler-model range in `text`, absent when no byte offset
/// answers its ends.
fn project_range(text: &ProjectedText, range: &SourceRange) -> Option<Range> {
    let offset = |at: &SourcePosition| {
        source_byte_offset(
            &text.lines,
            Utf16Position::new(
                u32::try_from(at.line).ok()?,
                u32::try_from(at.character).ok()?,
            ),
        )
        .ok()
    };
    let start = offset(&range.start)?;
    let end = offset(&range.end)?;
    crate::position::utf16_range_indexed(&text.lines, start..end, text.first_break)
}

/// The protocol range one diagnostic location addresses.
///
/// A range a producer retained, and one inferred from its line and column, both hold the
/// compiler's line model: each end's byte offset is read in that model and the position the client
/// addresses is taken from the same bytes, so a source whose lines break differently lands on the
/// line the client counts. A range no text answers keeps the producer's own coordinates while the
/// protocol can hold them and they run forwards; coordinates beyond the protocol's own bounds, and
/// ones that run backwards, anchor at the document's first byte, because the publish that carries
/// every other diagnostic is worth more than the position of one.
fn diagnostic_range(
    location: Option<&Location>,
    path: Option<&Path>,
    texts: &mut DiagnosticTexts<'_>,
) -> Range {
    let Some(location) = location else {
        return Range::default();
    };
    let range = location
        .range
        .unwrap_or_else(|| inferred_source_range(location, path, texts));
    // A range that names the document's own first byte is the same in either model.
    if range.start == range.end && range.start.line == 0 && range.start.character == 0 {
        return Range::default();
    }
    if let Some(projected) = path
        .and_then(|path| texts.text(path))
        .and_then(|text| project_range(text, &range))
    {
        return projected;
    }
    let start = position::checked_position(range.start.line, range.start.character)
        .unwrap_or_else(|| Position::new(0, 0));
    let end = position::checked_position(range.end.line, range.end.character)
        .unwrap_or_else(|| Position::new(0, 0));
    if end < start {
        Range::new(start, start)
    } else {
        Range::new(start, end)
    }
}

/// The range a location that retained none addresses.
///
/// A column counts characters while the protocol counts UTF-16 code units, so the line's own text
/// is what converts one to the other: the excerpt the location has, or the line of the file it
/// names, in the compiler's own line model. A line neither source reaches answers at its start,
/// which claims nothing about the column the producer wrote.
fn inferred_source_range(
    location: &Location,
    path: Option<&Path>,
    texts: &mut DiagnosticTexts<'_>,
) -> SourceRange {
    let line = location.line.unwrap_or(1).saturating_sub(1);
    let column = location.column.unwrap_or(1).saturating_sub(1);
    let excerpt = location.source_lines.iter().find(|excerpt| {
        excerpt.line == line + 1
            && excerpt.start_column <= column
            && column <= excerpt.start_column + excerpt.text.chars().count()
    });
    // The excerpt is the text its producer meant; only a location that retained no excerpt and a
    // column needs the file's own line read to convert it.
    let file_line = if location.source_lines.is_empty() && location.column.is_some() {
        path.and_then(|path| texts.text(path))
            .and_then(|text| line_text(&text.lines, line))
    } else {
        None
    };
    let text = excerpt
        .map(|excerpt| excerpt.text.as_str())
        .or(file_line.as_deref());
    let start_character = excerpt.map_or(0, |excerpt| excerpt.start_character);
    let (start, end) = match text {
        Some(text) => match excerpt.and_then(|excerpt| excerpt.highlight) {
            Some((start, end)) => (
                start_character + position::utf16_len(&text[..start]),
                start_character + position::utf16_len(&text[..end]),
            ),
            None => {
                let start = start_character
                    + text
                        .chars()
                        .take(column - excerpt.map_or(0, |excerpt| excerpt.start_column))
                        .map(char::len_utf16)
                        .sum::<usize>();
                (start, start)
            }
        },
        None => (0, 0),
    };
    SourceRange {
        start: SourcePosition {
            line,
            character: start,
        },
        end: SourcePosition {
            line,
            character: end,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::tests::Site;
    use lsp_types::Position;
    use tola_diagnostic::DiagnosticCause;

    /// One location a producer named without its text.
    fn located_at(path: &str, line: usize, column: usize) -> Location {
        Location {
            path: path.into(),
            line: Some(line),
            column: Some(column),
            range: None,
            source_lines: Vec::new(),
        }
    }

    /// One diagnostic as the projection reads it, through the site the test built.
    fn project(site: &Site, diagnostic: Diagnostic) -> Option<(PathBuf, EditorDiagnostic)> {
        let mut texts = DiagnosticTexts::new(&site.open);
        editor_diagnostic(
            diagnostic,
            &site.physical,
            &site.physical.join("site.typ"),
            &crate::uri::ClientRoot::new(&site.physical),
            false,
            &mut texts,
        )
        .unwrap()
    }

    /// One projected diagnostic, which every case in this module expects to be alone.
    fn alone(site: &Site) -> lsp_types::Diagnostic {
        let diagnostics = site.diagnostics();
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("one diagnostic: {diagnostics:?}");
        };
        diagnostic.clone()
    }

    /// A range the protocol cannot hold anchors where the report began, so the publish that carries
    /// every other diagnostic still happens.
    #[test]
    fn unrepresentable_range_anchors_at_start() {
        for (start, end, anchored) in [
            (
                SourcePosition {
                    line: usize::MAX,
                    character: 0,
                },
                SourcePosition {
                    line: usize::MAX,
                    character: 3,
                },
                Position::new(0, 0),
            ),
            (
                SourcePosition {
                    line: 1,
                    character: 4,
                },
                SourcePosition {
                    line: 0,
                    character: 0,
                },
                Position::new(1, 4),
            ),
        ] {
            let mut site = Site::new(&[("content/page.typ", "Body\n")]);
            site.publish(vec![
                Diagnostic::at_path(
                    tola_diagnostic::codes::typst::COMPILE,
                    Severity::Error,
                    "content/page.typ",
                    "unrepresentable span",
                )
                .with_source_range(SourceRange { start, end }),
            ]);
            let diagnostic = alone(&site);
            assert_eq!(diagnostic.range, Range::new(anchored, anchored));
        }
    }

    /// A diagnostic after a line separator the compiler counts and the client does not lands on the
    /// client's own line: the range is projected through the protocol's own line model.
    #[test]
    fn line_separator_projects_client_line() {
        let mut site = Site::new(&[("content/page.typ", "one\u{2028}two\nthree\n")]);
        site.publish(vec![
            Diagnostic::at_path(
                tola_diagnostic::codes::typst::COMPILE,
                Severity::Error,
                "content/page.typ",
                "source error",
            )
            .with_source_range(SourceRange {
                start: SourcePosition {
                    line: 1,
                    character: 0,
                },
                end: SourcePosition {
                    line: 1,
                    character: 3,
                },
            }),
        ]);
        let diagnostic = alone(&site);
        assert_eq!(
            diagnostic.range,
            Range::new(Position::new(0, 4), Position::new(0, 7))
        );
    }
    /// Advice kept out of the message travels as related information when the client reads it, and
    /// folds back into the message when it does not: the same text reaches exactly one channel.
    #[test]
    fn advice_takes_exactly_one_channel() {
        struct Case {
            name: &'static str,
            advice: &'static str,
            location: Option<Location>,
            expect_related: bool,
        }
        let cases = [
            Case {
                name: "site-located help",
                advice: "pass the page title",
                location: Some(located_at("content/page.typ", 1, 1)),
                expect_related: true,
            },
            Case {
                name: "site-located trace",
                advice: "while calling `select-pages`",
                location: Some(located_at("content/page.typ", 1, 1)),
                expect_related: true,
            },
            Case {
                name: "package-located help",
                advice: "pass the page title",
                location: Some(located_at("@tola/source:0.0.0/lib.typ", 93, 7)),
                expect_related: false,
            },
            Case {
                name: "unlocated trace",
                advice: "while calling an anonymous function",
                location: None,
                expect_related: false,
            },
        ];
        for case in cases {
            for related in [false, true] {
                let mut site = Site::new(&[("content/page.typ", "Body\n")]);
                let mut diagnostic = Diagnostic::at_path(
                    tola_diagnostic::codes::typst::COMPILE,
                    Severity::Error,
                    "content/page.typ",
                    "primary failure",
                );
                let help = tola_diagnostic::Help {
                    message: case.advice.into(),
                    location: case.location.clone(),
                };
                diagnostic.help.push(help);
                if related {
                    site.publish_with(&crate::diagnostic::tests::features(), vec![diagnostic]);
                } else {
                    site.publish(vec![diagnostic]);
                }
                let diagnostic = alone(&site);
                let in_message = diagnostic.message.matches(case.advice).count();
                let in_related = diagnostic
                    .related_information
                    .iter()
                    .flatten()
                    .filter(|info| info.message == case.advice)
                    .count();
                let routed = related && case.expect_related;
                assert_eq!(
                    (in_message, in_related),
                    if routed { (0, 1) } else { (1, 0) },
                    "{} (related information {related})",
                    case.name
                );
            }
        }
    }
    /// A source metadata issue names a field, not a site call: the editor does not surface the
    /// parse pipeline that reached it.
    #[test]
    fn source_metadata_omits_the_parse_trace() {
        let mut site = Site::new(&[("content/page.typ", "Body\n")]);
        let mut diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::source::METADATA,
            Severity::Error,
            "content/page.typ",
            "`$.published` is not set",
        );
        diagnostic.trace.push(tola_diagnostic::TraceFrame {
            message: "while calling `select-pages`".into(),
            location: Some(located_at("site/pages.typ", 2, 2)),
        });
        site.publish_with(&crate::diagnostic::tests::features(), vec![diagnostic]);
        let diagnostic = alone(&site);
        assert!(!diagnostic.message.contains("select-pages"));
        assert!(diagnostic.related_information.is_none());
    }
    /// A cause a client echoes back travels in `data`, so a correction can read it back.
    #[test]
    fn causes_travel_in_diagnostic_data() {
        let mut site = Site::new(&[("content/page.typ", "Body\n")]);
        site.publish(vec![
            Diagnostic::at_path(
                tola_diagnostic::codes::typst::COMPILE,
                Severity::Error,
                "content/page.typ",
                "unknown variable",
            )
            .with_cause(DiagnosticCause::UnknownVariable {
                name: "missing".into(),
            }),
        ]);
        let diagnostic = alone(&site);
        assert_eq!(
            diagnostic.data,
            Some(serde_json::json!({ "kind": "unknown-variable", "name": "missing" }))
        );
    }
    /// A report the site declares reaches the editor with its own level and code, projected
    /// through the unsaved buffer that declared it.
    #[test]
    fn public_projects_through_unsaved_buffer() {
        use std::sync::Arc;
        use tola_build::{cancellation::BuildCancellation, check::SourceDiagnosticSession};
        let mut site = Site::new(&[
            (
                "site.typ",
                "#document(\"index.html\")[#include \"content/page.typ\"]",
            ),
            ("content/page.typ", "Saved body"),
            ("tola.toml", ""),
        ]);
        let overlay = "#import \"@tola/diagnostic:0.0.0\": report\n#let value = emph[unsaved 🧭]\n#report(\"unknown variable\", severity: \"info\", code: \"typst.compile/中文\", at: value, hints: (\"Correction\",), related: ((message: \"Context\", at: value), (message: \"Unlocated context\", at: none)))";
        site.open("content/page.typ", overlay);
        let config = crate::compiler::tests::site_configuration(&site.physical, "");
        let mut session = SourceDiagnosticSession::new(config);
        let revision = session
            .inspect(
                vec![(site.physical.join("content/page.typ"), Arc::from(overlay))],
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(
            revision.compiled(),
            "info does not fail checking: {:?}",
            revision.diagnostics()
        );
        site.publish_with(
            &crate::diagnostic::tests::features(),
            revision.into_diagnostics(),
        );
        let report = site
            .diagnostics()
            .into_iter()
            .find(|diagnostic| diagnostic.source.as_deref() == Some("project"))
            .expect("public issue reaches the open source");
        assert_eq!(report.severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(
            report.code,
            Some(NumberOrString::String("typst.compile/中文".into()))
        );
        assert!(report.data.is_none(), "report text cannot infer a core fix");
        assert_eq!(report.range.start.line, 1);
        assert!(report.range.end.character > report.range.start.character);
        assert_eq!(report.related_information.as_ref().unwrap().len(), 1);
        assert!(report.message.contains("help: Correction"));
        assert!(report.message.contains("related: Unlocated context"));
        assert_eq!(
            std::fs::read_to_string(site.physical.join("content/page.typ")).unwrap(),
            "Saved body"
        );
    }

    #[test]
    fn unlocated_reports_are_not_diagnostics() {
        let site = Site::new(&[("content/page.typ", "Body")]);
        let mut diagnostic = Diagnostic::new(
            tola_diagnostic::codes::report::ISSUE,
            Severity::Error,
            "unlocated issue",
        );
        diagnostic.producer = Some(tola_diagnostic::DiagnosticProducer {
            name: Some("project".into()),
            code: None,
        });
        assert!(project(&site, diagnostic).is_none());
    }
    #[test]
    fn external_never_editor_file_mappings() {
        let site = Site::new(&[("content/page.typ", "Actual editor text")]);
        let mut diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::report::ISSUE,
            Severity::Warning,
            "content/page.typ",
            "external issue",
        );
        diagnostic.producer = Some(tola_diagnostic::DiagnosticProducer {
            name: Some("command:catalog".into()),
            code: Some("rule".into()),
        });
        diagnostic.location.as_mut().unwrap().range = Some(SourceRange {
            start: SourcePosition {
                line: 0,
                character: 0,
            },
            end: SourcePosition {
                line: 0,
                character: 1,
            },
        });
        let evidence = DiagnosticSource::new(
            tola_typst::ReadLocator::NonPersistent("snapshot-only".into()),
            tola_typst::ContentDigest::of(b"Actual editor text"),
        );
        diagnostic.source = Some(evidence.clone());
        assert!(project(&site, diagnostic.clone()).is_none());
        let actual = site.physical.join("content/page.typ");
        diagnostic.source = Some(evidence.with_editor_path(actual.clone()));
        diagnostic.location.as_mut().unwrap().path = "Shared human-readable label".into();
        diagnostic
            .related
            .push(tola_diagnostic::RelatedInformation {
                message: "unmapped related snapshot".into(),
                location: diagnostic.location.clone(),
                source: Some(DiagnosticSource::new(
                    tola_typst::ReadLocator::NonPersistent("related-only".into()),
                    tola_typst::ContentDigest::of(b"Actual editor text"),
                )),
            });
        let (path, projected) = project(&site, diagnostic).unwrap();
        assert_eq!(path, actual);
        assert!(projected.related_information.is_none());
        assert!(
            projected
                .message
                .contains("related: unmapped related snapshot")
        );
    }

    #[test]
    fn unlocated_report_ignores_trace_location() {
        let site = Site::new(&[("content/page.typ", "Body")]);
        let mut diagnostic = Diagnostic::new(
            tola_diagnostic::codes::report::ISSUE,
            Severity::Warning,
            "unlocated report",
        );
        diagnostic.producer = Some(tola_diagnostic::DiagnosticProducer {
            name: Some("project".into()),
            code: None,
        });
        let mut location = located_at("content/page.typ", 1, 1);
        location.range = Some(SourceRange {
            start: SourcePosition {
                line: 0,
                character: 0,
            },
            end: SourcePosition {
                line: 0,
                character: 1,
            },
        });
        diagnostic.trace.push(tola_diagnostic::TraceFrame {
            message: "real caller".into(),
            location: Some(location),
        });
        assert!(project(&site, diagnostic).is_none());
    }
    #[test]
    fn external_ranges_need_current_digest() {
        let mut site = Site::new(&[("content/page.typ", "abc")]);
        site.open("content/page.typ", "🦊abc");
        let path = site.physical.join("content/page.typ");
        let mut diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::report::ISSUE,
            Severity::Warning,
            "snapshot label",
            "checked source",
        );
        diagnostic.producer = Some(tola_diagnostic::DiagnosticProducer {
            name: Some("checker".into()),
            code: Some("rule".into()),
        });
        diagnostic.location.as_mut().unwrap().range = Some(SourceRange {
            start: SourcePosition {
                line: 0,
                character: 0,
            },
            end: SourcePosition {
                line: 0,
                character: 3,
            },
        });
        let source = |text: &str| {
            DiagnosticSource::new(
                tola_typst::ReadLocator::ProvidedRoot("content/page.typ".into()),
                tola_typst::ContentDigest::of(text.as_bytes()),
            )
            .with_editor_path(path.clone())
        };
        diagnostic.source = Some(source("abc"));
        assert!(
            project(&site, diagnostic.clone()).is_none(),
            "a saved-file digest cannot authorize a range on a changed open buffer"
        );
        let old_location = diagnostic.location.clone();
        diagnostic.source = Some(source("🦊abc"));
        diagnostic.location.as_mut().unwrap().range = Some(SourceRange {
            start: SourcePosition {
                line: 0,
                character: 2,
            },
            end: SourcePosition {
                line: 0,
                character: 5,
            },
        });
        diagnostic
            .related
            .push(tola_diagnostic::RelatedInformation {
                message: "old related range".into(),
                location: old_location,
                source: Some(source("abc")),
            });
        let (_, projected) = project(&site, diagnostic).unwrap();
        assert_eq!(
            projected.range,
            Range::new(Position::new(0, 2), Position::new(0, 5))
        );
        assert!(
            projected.related_information.is_none(),
            "related positions also need matching input evidence"
        );
        assert!(projected.message.contains("related: old related range"));
    }
}
