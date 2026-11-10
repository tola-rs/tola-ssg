//! Presentation-free diagnostics shared by producers and output adapters.
//!
//! The optional `json-schema` feature implements `schemars::JsonSchema` for every
//! record, using their Serde serialization contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod code;
pub mod codes;
mod error;
mod path;
mod positions;
mod repeat;
mod report;
mod source;

pub use code::DiagnosticCode;
pub use error::{DiagnosticError, attached, collapse_whitespace, fallback};
pub use path::{display_path, render_relative};
pub use positions::{SourcePositions, key_at_span};
pub use repeat::{LISTED_NAMES, ReferenceIdentity, bounded_listing, collapse_repeated};
pub use report::{coalesce_reports, ensure_reports_succeed};
pub use source::{
    Location, SourceExcerpt, SourceLine, SourcePosition, SourceRange, line_column,
    unparsable_source, unreadable_source,
};

use serde::Serialize;

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
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
    fn unread_import_wire() {
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
}
