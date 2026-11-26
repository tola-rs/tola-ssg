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
pub mod toml;

pub use code::DiagnosticCode;
pub use error::{DiagnosticError, attached, collapse_whitespace, fallback};
pub use path::{display_path, render_relative};
pub use positions::{SourcePositions, WrittenPosition, key_at_span, push_pointer};
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

/// Cause classified by a diagnostic producer.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DiagnosticCause {
    UnknownConfigurationFields {
        fields: Vec<UnknownField>,
    },
    UnknownVariable {
        name: String,
    },
    UnresolvedReference {
        destination: String,
        replacements: Vec<String>,
    },
    UnreadImport {
        names: Vec<String>,
        removal: UnreadRemoval,
    },
}

/// Edit range for an unread import.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UnreadRemoval {
    /// One-based line of the whole import statement.
    Statement { line: usize },
    /// Individual byte/UTF-16 spans for unread bindings.
    Spans { ranges: Vec<SourceRange> },
}

/// Unknown configuration key in TOML.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct UnknownField {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written: Option<SourceRange>,
}

/// Ownership and optional rule code of a diagnostic report.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticProducer {
    pub name: Option<String>,
    pub code: Option<String>,
}

/// Trusted input evidence retained across host wrapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticSource {
    locator: tola_typst::ReadLocator,
    digest: tola_typst::ContentDigest,
    editor_path: Option<std::path::PathBuf>,
    sha256: Option<tola_input::Sha256Digest>,
}

impl DiagnosticSource {
    pub fn new(locator: tola_typst::ReadLocator, digest: tola_typst::ContentDigest) -> Self {
        Self {
            locator,
            digest,
            editor_path: None,
            sha256: None,
        }
    }

    pub fn input_identity(&self) -> (&tola_typst::ReadLocator, tola_typst::ContentDigest) {
        (&self.locator, self.digest)
    }

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

    pub fn with_sha256(mut self, text: &str) -> Self {
        self.sha256 = Some(tola_input::Sha256Digest::of_bytes(text.as_bytes()));
        self
    }

    pub fn sha256(&self) -> Option<tola_input::Sha256Digest> {
        self.sha256
    }
}

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelatedInformation {
    pub message: String,
    pub location: Option<Location>,
    #[serde(skip)]
    #[cfg_attr(feature = "json-schema", schemars(skip))]
    pub source: Option<DiagnosticSource>,
}

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
    pub imported_by: Vec<String>,
    pub notes: Vec<String>,
    pub help: Vec<Help>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedInformation>,
    pub trace: Vec<TraceFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<DiagnosticCause>,
}

impl Diagnostic {
    pub fn new(code: DiagnosticCode, severity: Severity, msg: impl Into<String>) -> Self {
        let msg = msg.into();
        let message = if msg.trim().is_empty() {
            "Tola could not complete the operation".to_owned()
        } else {
            msg
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

    pub fn code_label(&self) -> String {
        let Some(prod) = &self.producer else {
            return self.code.to_string();
        };
        let name = prod.name.as_deref().unwrap_or("unknown");
        match &prod.code {
            Some(code) => format!("{name}: {code}"),
            None => name.to_owned(),
        }
    }

    pub fn at_path(
        code: DiagnosticCode,
        severity: Severity,
        path: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        Self::new(code, severity, msg).with_path(path)
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn display_message(&self) -> &str {
        if self.producer.is_some() {
            return &self.message;
        }
        self.message
            .strip_prefix("panicked with: ")
            .unwrap_or(&self.message)
    }

    pub fn distinct_notes(&self) -> impl Iterator<Item = &str> {
        distinct(&self.notes).map(String::as_str)
    }

    pub fn distinct_help(&self) -> impl Iterator<Item = &Help> {
        distinct(&self.help)
    }

    pub fn with_cause(mut self, cause: DiagnosticCause) -> Self {
        self.cause = Some(cause);
        self
    }

    pub fn at_location(
        code: DiagnosticCode,
        severity: Severity,
        loc: Location,
        msg: impl Into<String>,
    ) -> Self {
        let mut diag = Self::new(code, severity, msg);
        diag.location = Some(loc);
        diag
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.location = Some(Location::at_path(path));
        self
    }

    pub fn with_file(mut self, file: std::path::PathBuf) -> Self {
        self.location
            .as_mut()
            .expect("an editor file requires a display path")
            .file = Some(file);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help.push(Help {
            message: help.into(),
            location: None,
        });
        self
    }

    pub fn with_position(mut self, line: usize, col: usize) -> Self {
        assert!(line > 0, "a diagnostic line number must be one-based");
        assert!(col > 0, "a diagnostic column must be one-based");
        let loc = self
            .location
            .as_mut()
            .expect("a diagnostic position requires a diagnostic path");
        loc.line = Some(line);
        loc.column = Some(col);
        self
    }

    pub fn with_source_range(mut self, range: SourceRange) -> Self {
        let loc = self
            .location
            .as_mut()
            .expect("a diagnostic range requires a diagnostic path");
        loc.range = Some(range);
        self
    }

    pub fn with_source_span(self, text: &str, span: std::ops::Range<usize>) -> Self {
        let Some(excerpt) = SourceExcerpt::of(text, span.clone()) else {
            return self;
        };
        self.with_source_excerpt(
            excerpt.line,
            excerpt.column,
            excerpt.text,
            Some(excerpt.highlight),
        )
        .with_source_range(source::source_range(text, span))
    }

    pub fn with_source_excerpt(
        mut self,
        line: usize,
        col: usize,
        text: impl Into<String>,
        highlight: Option<(usize, usize)>,
    ) -> Self {
        let loc = self
            .location
            .as_mut()
            .expect("a source excerpt requires a diagnostic path");
        loc.source_lines = vec![SourceLine::new(line, text, highlight)];
        self.with_position(line, col)
    }
}

fn distinct<T: PartialEq>(items: &[T]) -> impl Iterator<Item = &T> {
    items
        .iter()
        .enumerate()
        .filter(|(idx, item)| !items[..*idx].contains(item))
        .map(|(_, item)| item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_import_wire() {
        let stmt = DiagnosticCause::UnreadImport {
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
            serde_json::to_value(&stmt).unwrap(),
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
