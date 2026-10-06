//! Presentation-free diagnostics shared by producers and output adapters.
//!
//! Producers classify errors; terminal and browser adapters render these records.

mod code;
mod error;

pub use code::DiagnosticCode;
pub use error::{DiagnosticError, attached, fallback};

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
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

/// A retained window of one source line. `highlight` contains zero-based, end-exclusive UTF-8 byte
/// offsets into `text`.
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
///
/// The caller passes the identity of the thing the author must change, so two pages that render
/// one mistake report once. Order follows first appearance.
pub(crate) fn collapse_repeated<K: std::hash::Hash + Eq>(
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

/// The identity of the source one record names; two references share it when one source
/// mistake reached several pages.
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct ReferenceIdentity {
    pub(crate) code: DiagnosticCode,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) column: Option<usize>,
    pub(crate) message: String,
}

impl ReferenceIdentity {
    pub(crate) fn of(diagnostic: &Diagnostic) -> Self {
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
///
/// One bound serves both the sentence a site author reads and the corrections that answer for the
/// same names, so a diagnostic and its fixes never disagree about which ones are offered.
pub(crate) const LISTED_NAMES: usize = 3;

/// Join names the way a sentence reads, naming [`LISTED_NAMES`] and counting the rest.
///
/// `unit` is the noun a counted remainder uses, in singular and plural form.
pub(crate) fn bounded_listing(names: &[String], unit: (&str, &str)) -> String {
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
    // The counted remainder is the sentence's final clause, so the names it follows join
    // with commas and the remainder has the `and`.
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
///
/// A correction reads those values — a configuration key's spelling and the line that writes it,
/// the name a source leaves unbound, the destination a reference writes — instead of the sentence
/// an author reads.
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
        /// The destination the source writes, which is the text a correction replaces. It sits
        /// inside the range the diagnostic reports, so a correction needs no search for it.
        destination: String,
        /// The destinations that would resolve, as a browser writes them, in the site's own order.
        ///
        /// One entry is the address the site publishes; several leave the choice to the author
        /// rather than guessing among them. Empty when the site publishes none.
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
///
/// The statement decides: one whose every bound name is unread goes whole, while one that also
/// binds a read name loses only the names no occurrence reads.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UnreadRemoval {
    /// The whole statement, on the one-based line that writes it.
    ///
    /// The line goes with its ending, so the names beside it cannot be left without a statement
    /// of their own.
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
    /// The one-based line that writes the key, absent when the document's own spelling cannot be
    /// found for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The exact span that writes the key, absent under the same condition. A correction replaces
    /// this span, so no editor searches the document for the key's spelling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written: Option<SourceRange>,
}

/// A diagnostic record shared by terminal and browser output.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub location: Option<Location>,
    /// Site files containing literal package imports or includes, not proven call sites.
    pub imported_by: Vec<String>,
    pub notes: Vec<String>,
    pub help: Vec<Help>,
    pub trace: Vec<TraceFrame>,
    /// The cause this diagnostic classified, absent when no correction applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<DiagnosticCause>,
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
            message,
            location: None,
            imported_by: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
            trace: Vec::new(),
            cause: None,
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
    ///
    /// A producer that knows where the failure is but does not hold the file text names the
    /// position; a renderer that can read the file shows it, and one that cannot still names it.
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
    ///
    /// A renderer that can read the file underlines the range; one that cannot still names its
    /// start through the position.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_references_count_distinct_pages() {
        for pages in [
            vec!["index.html", "index.html"],
            vec!["index.html", "about.html", "index.html"],
        ] {
            let diagnostic = Diagnostic::new(
                crate::codes::typst::COMPILE,
                Severity::Warning,
                "missing destination",
            );
            let grouped = collapse_repeated(
                pages
                    .iter()
                    .map(|page| (0, diagnostic.clone(), (*page).to_owned())),
            );
            assert_eq!(grouped.len(), 1);
            if pages.contains(&"about.html") {
                assert_eq!(grouped[0].notes.len(), 1);
                let note = &grouped[0].notes[0];
                assert_eq!(note.matches("index.html").count(), 1);
                assert_eq!(note.matches("about.html").count(), 1);
            } else {
                assert!(grouped[0].notes.is_empty());
            }
        }
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

    /// The wire shape a client parses back out of `Diagnostic::data`.
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
}
