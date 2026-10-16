//! Presentation-free diagnostics shared by producers and output adapters.
//!
//! The optional `json-schema` feature implements `schemars::JsonSchema` for every
//! record, using their Serde serialization contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod code;
pub mod codes;
mod error;

pub use code::DiagnosticCode;
pub use error::{DiagnosticError, attached, collapse_whitespace, fallback};

use serde::{Deserialize, Serialize};
use typst::syntax::Lines;

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

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
    fn at_path(path: impl Into<String>) -> Self {
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

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Help {
    pub message: String,
    pub location: Option<Location>,
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceFrame {
    pub message: String,
    pub location: Option<Location>,
}

/// Collapse the records that one source mistake produced, naming the pages each reached.
pub fn collapse_repeated<K: std::hash::Hash + Eq>(
    diagnostics: impl IntoIterator<Item = (K, Diagnostic, String)>,
) -> Vec<Diagnostic> {
    let mut positions: std::collections::HashMap<K, usize> = std::collections::HashMap::new();
    let mut groups: Vec<(Diagnostic, Vec<String>)> = Vec::new();
    for (key, diagnostic, page) in diagnostics {
        match positions.get(&key) {
            Some(&index) => {
                let pages = &mut groups[index].1;
                if !pages.contains(&page) {
                    pages.push(page);
                }
            }
            None => {
                positions.insert(key, groups.len());
                groups.push((diagnostic, vec![page]));
            }
        }
    }
    groups
        .into_iter()
        .map(|(diagnostic, pages)| match pages_note(&pages) {
            Some(note) => diagnostic.with_note(note),
            None => diagnostic,
        })
        .collect()
}

/// The identity of the source one record names, shared across pages.
#[derive(PartialEq, Eq, Hash)]
pub struct ReferenceIdentity {
    pub(crate) code: DiagnosticCode,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) column: Option<usize>,
    pub(crate) message: String,
}

impl ReferenceIdentity {
    pub fn of(diagnostic: &Diagnostic) -> Self {
        let location = diagnostic.location.as_ref();
        Self {
            code: diagnostic.code,
            path: location.map(|location| location.path.clone()),
            line: location.and_then(|location| location.line),
            column: location.and_then(|location| location.column),
            message: diagnostic.message.clone(),
        }
    }
}

/// One sentence naming the pages a repeated record reached, counting beyond the first three.
fn pages_note(pages: &[String]) -> Option<String> {
    match pages {
        [] | [_] => None,
        pages => Some(format!(
            "it appears on {}",
            bounded_listing(pages, ("page", "pages"))
        )),
    }
}

/// How many names a listing names before it counts the rest.
pub const LISTED_NAMES: usize = 3;

/// Join names the way a sentence reads, naming [`LISTED_NAMES`] and counting the rest.
pub fn bounded_listing(names: &[String], unit: (&str, &str)) -> String {
    let quoted = quoted_names(names);
    if names.len() <= LISTED_NAMES {
        return sentence_list(&quoted);
    }
    let remaining = names.len() - LISTED_NAMES;
    let (singular, plural) = unit;
    let rest = if remaining == 1 {
        format!("1 more {singular}")
    } else {
        format!("{remaining} more {plural}")
    };
    // The counted remainder is the sentence's final clause and carries the `and`.
    format!("{} and {rest}", quoted[..LISTED_NAMES].join(", "))
}

pub(crate) fn quoted_names(names: &[String]) -> Vec<String> {
    names.iter().map(|name| format!("`{name}`")).collect()
}

/// Join names the way a sentence reads: `a`, `b` and `c`.
pub(crate) fn sentence_list(names: &[String]) -> String {
    let Some((last, head)) = names.split_last() else {
        return String::new();
    };
    if head.is_empty() {
        return last.clone();
    }
    format!("{} and {last}", head.join(", "))
}

/// The cause a diagnostic classified, with the values its producer already knows.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DiagnosticCause {
    /// Configuration keys the schema does not know, as the document spells them.
    UnknownConfigurationFields { fields: Vec<UnknownField> },
    /// The name one source reads without binding it.
    UnknownVariable { name: String },
    /// One reference whose destination the site does not resolve.
    UnresolvedReference {
        /// The destination the source writes, which is the text a correction replaces.
        destination: String,
        /// The destinations that would resolve, as a browser writes them, in the site's own order.
        replacements: Vec<String>,
    },
    /// One import statement binding names its source never reads.
    UnreadImport {
        /// The bound names no occurrence reads, in the order the statement binds them.
        names: Vec<String>,
        /// What a correction removes, which the producer reads from the statement's own text.
        removal: UnreadRemoval,
    },
}

/// What a correction removes for one unread import.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UnreadRemoval {
    /// The whole statement, on the one-based line that writes it.
    Statement { line: usize },
    /// The spans that write the unread names, each removed on its own.
    Spans { ranges: Vec<SourceRange> },
}

/// One configuration key the schema does not know.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct UnknownField {
    /// The key as the document spells it, including the tables it sits under.
    pub name: String,
    /// The one-based line that writes the key, when the document spells it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The exact span that writes the key, absent under the same condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written: Option<SourceRange>,
}

/// Host-established ownership of a public report and its optional producer-owned rule code.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticProducer {
    /// Project or full package identity, absent when the native caller has no source identity.
    pub name: Option<String>,
    pub code: Option<String>,
}

/// Trusted input evidence retained across host error wrapping and presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticSource {
    locator: tola_typst::ReadLocator,
    digest: tola_typst::ContentDigest,
    editor_path: Option<std::path::PathBuf>,
}

impl DiagnosticSource {
    pub fn new(locator: tola_typst::ReadLocator, digest: tola_typst::ContentDigest) -> Self {
        Self {
            locator,
            digest,
            editor_path: None,
        }
    }

    pub fn input_identity(&self) -> (&tola_typst::ReadLocator, tola_typst::ContentDigest) {
        (&self.locator, self.digest)
    }

    /// Attach an actual host-resolved editor file, independently of the display label.
    pub fn with_editor_path(mut self, path: std::path::PathBuf) -> Self {
        assert!(
            path.is_absolute(),
            "a host diagnostic editor path must be absolute"
        );
        self.editor_path = Some(path);
        self
    }

    pub fn editor_path(&self) -> Option<&std::path::Path> {
        self.editor_path.as_deref()
    }
}

/// Context accompanying an issue, distinct from advice about correcting it.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelatedInformation {
    pub message: String,
    pub location: Option<Location>,
    #[serde(skip)]
    #[cfg_attr(feature = "json-schema", schemars(skip))]
    pub source: Option<DiagnosticSource>,
}

/// A diagnostic record shared by terminal and browser output.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<DiagnosticProducer>,
    #[serde(skip)]
    #[cfg_attr(feature = "json-schema", schemars(skip))]
    pub source: Option<DiagnosticSource>,
    pub message: String,
    pub location: Option<Location>,
    /// Site files containing literal package imports or includes, not proven call sites.
    pub imported_by: Vec<String>,
    pub notes: Vec<String>,
    pub help: Vec<Help>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedInformation>,
    /// The call stack behind this diagnostic, innermost first, as the terminal shows it.
    pub trace: Vec<TraceFrame>,
    /// The cause this diagnostic classified, absent when no correction applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<DiagnosticCause>,
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

impl Diagnostic {
    pub fn new(code: DiagnosticCode, severity: Severity, message: impl Into<String>) -> Self {
        let message = message.into();
        let message = if message.trim().is_empty() {
            "Tola could not complete the operation".to_owned()
        } else {
            message
        };
        Self {
            severity,
            code,
            producer: None,
            source: None,
            message,
            location: None,
            imported_by: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
            related: Vec::new(),
            trace: Vec::new(),
            cause: None,
        }
    }

    /// Display identity without treating a producer's rule text as a core code.
    pub fn code_label(&self) -> String {
        let Some(producer) = &self.producer else {
            return self.code.to_string();
        };
        let name = producer.name.as_deref().unwrap_or("unknown");
        match &producer.code {
            Some(code) => format!("{name}: {code}"),
            None => name.to_owned(),
        }
    }

    pub fn at_path(
        code: DiagnosticCode,
        severity: Severity,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(code, severity, message).with_path(path)
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Present the site's panic sentence while retaining Typst's original message in the record.
    pub fn display_message(&self) -> &str {
        if self.producer.is_some() {
            return &self.message;
        }
        self.message
            .strip_prefix("panicked with: ")
            .unwrap_or(&self.message)
    }

    /// Note text in encounter order, omitting exact repeats without changing the stored record.
    pub fn distinct_notes(&self) -> impl Iterator<Item = &str> {
        distinct(&self.notes).map(String::as_str)
    }

    /// Advice in encounter order. Equal text at different locations remains distinct.
    pub fn distinct_help(&self) -> impl Iterator<Item = &Help> {
        distinct(&self.help)
    }

    /// The cause a correction reads, which the rendered message is not.
    pub fn with_cause(mut self, cause: DiagnosticCause) -> Self {
        self.cause = Some(cause);
        self
    }

    /// A diagnostic located exactly where its producer found it.
    pub fn at_location(
        code: DiagnosticCode,
        severity: Severity,
        location: Location,
        message: impl Into<String>,
    ) -> Self {
        let mut diagnostic = Self::new(code, severity, message);
        diagnostic.location = Some(location);
        diagnostic
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.location = Some(Location::at_path(path));
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help.push(Help {
            message: help.into(),
            location: None,
        });
        self
    }

    /// Name the position inside the located file, without its text.
    pub fn with_position(mut self, line: usize, column: usize) -> Self {
        assert!(line > 0, "a diagnostic line number must be one-based");
        assert!(column > 0, "a diagnostic column must be one-based");
        let location = self
            .location
            .as_mut()
            .expect("a diagnostic position requires a diagnostic path");
        location.line = Some(line);
        location.column = Some(column);
        self
    }

    /// Name the exact range inside the located file, when the producer retained one.
    pub fn with_source_range(mut self, range: SourceRange) -> Self {
        let location = self
            .location
            .as_mut()
            .expect("a diagnostic range requires a diagnostic path");
        location.range = Some(range);
        self
    }

    pub fn with_source_excerpt(
        mut self,
        line: usize,
        column: usize,
        text: impl Into<String>,
        highlight: Option<(usize, usize)>,
    ) -> Self {
        let text = text.into();
        let location = self
            .location
            .as_mut()
            .expect("a source excerpt requires a diagnostic path");
        location.source_lines = vec![SourceLine::new(line, text, highlight)];
        self.with_position(line, column)
    }
}

fn distinct<T: PartialEq>(messages: &[T]) -> impl Iterator<Item = &T> {
    messages
        .iter()
        .enumerate()
        .filter(|(index, message)| !messages[..*index].contains(message))
        .map(|(_, message)| message)
}

/// Merge repeated public issues from this one source snapshot without losing their context.
pub fn coalesce_reports(diagnostics: &mut Vec<Diagnostic>) {
    let mut merged: Vec<Diagnostic> = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics.drain(..) {
        let same = diagnostic.producer.as_ref().and_then(|_| {
            let location = diagnostic.location.as_ref()?;
            let range = location.range?;
            merged.iter_mut().find(|existing| {
                existing.producer == diagnostic.producer
                    && existing.severity == diagnostic.severity
                    && existing.message == diagnostic.message
                    && existing.location.as_ref().is_some_and(|at| {
                        let same_source = match (&existing.source, &diagnostic.source) {
                            (Some(left), Some(right)) => {
                                left.input_identity() == right.input_identity()
                            }
                            (None, None) => at.path == location.path,
                            _ => false,
                        };
                        same_source && at.range == Some(range)
                    })
            })
        });
        if let Some(existing) = same {
            merge_distinct(&mut existing.help, diagnostic.help);
            merge_distinct(&mut existing.related, diagnostic.related);
            merge_distinct(&mut existing.notes, diagnostic.notes);
            merge_distinct(&mut existing.trace, diagnostic.trace);
            merge_distinct(&mut existing.imported_by, diagnostic.imported_by);
        } else {
            merged.push(diagnostic);
        }
    }
    *diagnostics = merged;
}

fn merge_distinct<T: PartialEq>(retained: &mut Vec<T>, incoming: impl IntoIterator<Item = T>) {
    for item in incoming {
        if !retained.contains(&item) {
            retained.push(item);
        }
    }
}

/// Accumulated reports finish evaluation normally but prevent a successful host operation.
pub fn ensure_reports_succeed(diagnostics: &[Diagnostic]) -> anyhow::Result<()> {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.producer.is_some() && diagnostic.severity == Severity::Error)
    {
        return Err(DiagnosticError::new(
            "a reported error prevents this operation",
            diagnostics.to_vec(),
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_mistake_names_each_page() {
        let diagnostic = Diagnostic::new(
            crate::codes::typst::COMPILE,
            Severity::Warning,
            "missing destination",
        );
        let grouped = collapse_repeated(
            ["index.html", "index.html", "about.html"]
                .map(|page| (0, diagnostic.clone(), page.to_owned())),
        );

        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].notes.len(), 1);
        assert_eq!(
            grouped[0].notes[0],
            "it appears on `index.html` and `about.html`"
        );
    }

    #[test]
    fn pages_note_counts_past_three() {
        let pages = |count: usize| {
            (0..count)
                .map(|index| format!("page-{index}.html"))
                .collect::<Vec<_>>()
        };

        assert_eq!(pages_note(&pages(0)), None);
        assert_eq!(pages_note(&pages(1)), None);
        assert_eq!(
            pages_note(&pages(2)).as_deref(),
            Some("it appears on `page-0.html` and `page-1.html`")
        );
        assert_eq!(
            pages_note(&pages(3)).as_deref(),
            Some("it appears on `page-0.html`, `page-1.html` and `page-2.html`")
        );
        assert_eq!(
            pages_note(&pages(4)).as_deref(),
            Some("it appears on `page-0.html`, `page-1.html`, `page-2.html` and 1 more page")
        );
        assert_eq!(
            pages_note(&pages(6)).as_deref(),
            Some("it appears on `page-0.html`, `page-1.html`, `page-2.html` and 3 more pages")
        );
    }

    #[test]
    fn diagnostic_serializes_its_code() {
        let diagnostic = Diagnostic::new(
            crate::codes::config::INVALID,
            Severity::Error,
            "invalid output directory",
        );

        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["code"], "config.invalid");
    }

    /// The wire shape a client reads from a diagnostic's `cause`.
    #[test]
    fn unread_import_serializes_its_removal() {
        let statement = DiagnosticCause::UnreadImport {
            names: vec!["link".to_owned()],
            removal: UnreadRemoval::Statement { line: 7 },
        };
        let spans = DiagnosticCause::UnreadImport {
            names: vec!["pick".to_owned()],
            removal: UnreadRemoval::Spans {
                ranges: vec![SourceRange {
                    start: SourcePosition {
                        line: 2,
                        character: 0,
                    },
                    end: SourcePosition {
                        line: 2,
                        character: 4,
                    },
                }],
            },
        };

        assert_eq!(
            serde_json::to_value(&statement).unwrap(),
            serde_json::json!({
                "kind": "unread-import",
                "names": ["link"],
                "removal": {"kind": "statement", "line": 7},
            })
        );
        assert_eq!(
            serde_json::to_value(&spans).unwrap(),
            serde_json::json!({
                "kind": "unread-import",
                "names": ["pick"],
                "removal": {
                    "kind": "spans",
                    "ranges": [{
                        "start": {"line": 2, "character": 0},
                        "end": {"line": 2, "character": 4},
                    }],
                },
            })
        );
    }

    #[test]
    fn invalid_byte_ranges_are_dropped() {
        let valid = SourceLine::new(1, "éx", Some((2, 3)));
        let inside_character = SourceLine::new(1, "éx", Some((1, 2)));
        let empty = SourceLine::new(1, "éx", Some((2, 2)));

        assert_eq!(valid.highlight, Some((2, 3)));
        assert_eq!(inside_character.highlight, None);
        assert_eq!(empty.highlight, None);
    }

    #[test]
    fn blank_message_uses_fallback_sentence() {
        let diagnostic = Diagnostic::new(crate::codes::typst::COMPILE, Severity::Error, "\n");

        assert_eq!(diagnostic.message, "Tola could not complete the operation");
    }

    #[test]
    fn byte_offset_reports_position() {
        assert_eq!(line_column("ab\ncd\n", 3), Some((2, 1)));
        assert_eq!(line_column("ab\n", 3), Some((2, 1)));
        assert_eq!(line_column("ab", 3), None);
    }

    fn report(owner: &str, code: &str) -> Diagnostic {
        let mut diagnostic = Diagnostic::at_path(
            crate::codes::report::ISSUE,
            Severity::Warning,
            "content/a.typ",
            "one issue",
        );
        diagnostic.location.as_mut().unwrap().range = Some(SourceRange {
            start: SourcePosition {
                line: 1,
                character: 3,
            },
            end: SourcePosition {
                line: 1,
                character: 5,
            },
        });
        diagnostic.producer = Some(DiagnosticProducer {
            name: Some(owner.into()),
            code: Some(code.into()),
        });
        diagnostic
    }

    #[test]
    fn coalescing_keeps_all_context() {
        let mut first = report("project", "typst.compile");
        first.help.push(Help {
            message: "first correction".into(),
            location: None,
        });
        let mut second = report("project", "typst.compile");
        second.help.push(Help {
            message: "second correction".into(),
            location: None,
        });
        second.related.push(RelatedInformation {
            message: "context".into(),
            location: None,
            source: None,
        });
        let mut diagnostics = vec![
            first,
            second.clone(),
            second,
            report("@tola-modules/other:0.0.0", "typst.compile"),
            report("project", "different"),
        ];
        coalesce_reports(&mut diagnostics);
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(diagnostics[0].help.len(), 2);
        assert_eq!(diagnostics[0].related.len(), 1);
        assert_eq!(diagnostics[0].code, crate::codes::report::ISSUE);
        assert_eq!(diagnostics[0].code_label(), "project: typst.compile");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.cause.is_none())
        );
    }

    #[test]
    fn public_message_wording_stays_literal() {
        let mut diagnostic = report("project", "rule");
        diagnostic.message = "panicked with: authored text".into();
        assert_eq!(diagnostic.display_message(), diagnostic.message);
    }
    #[test]
    fn wrapping_keeps_trusted_identity() {
        let evidence = |path: &str, bytes: &[u8]| {
            DiagnosticSource::new(
                tola_typst::ReadLocator::ProvidedRoot(path.into()),
                tola_typst::ContentDigest::of(bytes),
            )
        };
        let mut first = report("command:catalog", "same-rule");
        first.source = Some(evidence("first.typ", b"same source text"));
        first.help.push(Help {
            message: "first correction".into(),
            location: None,
        });
        let mut repeated = first.clone();
        repeated.help = vec![Help {
            message: "second correction".into(),
            location: None,
        }];
        let mut other_source = first.clone();
        other_source.source = Some(evidence("second.typ", b"same source text"));
        let mut other_revision = first.clone();
        other_revision.source = Some(evidence("first.typ", b"changed source text"));
        let mut unkeyed = first.clone();
        unkeyed.source = None;
        let error = DiagnosticError::new(
            "report errors",
            vec![first, repeated, other_source, other_revision, unkeyed],
        );
        let diagnostics = error.diagnostics();
        assert_eq!(
            diagnostics.len(),
            4,
            "display path and range cannot merge distinct inputs"
        );
        assert_eq!(
            diagnostics[0].help.len(),
            2,
            "the same input still merges all context"
        );
        assert_eq!(diagnostics[0].location, diagnostics[1].location);
        assert_ne!(
            diagnostics[0].source.as_ref().unwrap().input_identity(),
            diagnostics[1].source.as_ref().unwrap().input_identity()
        );
        let public = serde_json::to_value(&diagnostics[0]).unwrap();
        assert!(
            public.get("source").is_none(),
            "host evidence is not a wire field"
        );
    }
}
