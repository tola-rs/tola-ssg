//! Serializable diagnostic DTOs, retained only until they are deleted.
//!
//! The live diagnostic already has every field these reshape, so nothing in this
//! workspace reads them; they stay for callers that serialized them across a boundary.
#![allow(deprecated)]

use serde::{Deserialize, Serialize};

use super::{ResolvedPackageFailure, ResolvedSource};
use crate::diagnostic::collection::Diagnostics;
use crate::diagnostic::message::{
    Diagnostic, Hint, LocationFailure, SourceLine, SourceTruncation, Trace, TraceKind,
};
use typst::diag::Severity;

/// Serializable diagnostic severity independent of Typst's internal types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolvedSeverity {
    /// A compilation error.
    Error,
    /// A non-fatal warning.
    Warning,
}

impl From<Severity> for ResolvedSeverity {
    fn from(value: Severity) -> Self {
        match value {
            Severity::Error => Self::Error,
            Severity::Warning => Self::Warning,
        }
    }
}

/// Why a source location could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedLocationFailure {
    /// The span is detached.
    DetachedSpan,
    /// Source loading failed.
    SourceUnavailable,
    /// No range was available.
    RangeUnavailable,
    /// The range was reversed.
    ReversedRange,
    /// The range exceeded source bounds.
    OutOfBounds,
    /// The range split a UTF-8 code point.
    InvalidUtf8Boundary,
}

impl From<LocationFailure> for ResolvedLocationFailure {
    fn from(value: LocationFailure) -> Self {
        match value {
            LocationFailure::DetachedSpan => Self::DetachedSpan,
            LocationFailure::SourceUnavailable => Self::SourceUnavailable,
            LocationFailure::RangeUnavailable => Self::RangeUnavailable,
            LocationFailure::ReversedRange => Self::ReversedRange,
            LocationFailure::OutOfBounds => Self::OutOfBounds,
            LocationFailure::InvalidUtf8Boundary => Self::InvalidUtf8Boundary,
        }
    }
}

/// Source-context truncation metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTruncation {
    /// Number of omitted lines.
    pub omitted_lines: usize,
    /// Number of omitted bytes.
    pub omitted_bytes: usize,
}

impl From<SourceTruncation> for ResolvedTruncation {
    fn from(value: SourceTruncation) -> Self {
        Self {
            omitted_lines: value.omitted_lines,
            omitted_bytes: value.omitted_bytes,
        }
    }
}

/// A resolved span, using zero-based columns for highlighting ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSpan {
    /// One-based starting line.
    pub start_line: usize,
    /// Zero-based starting column.
    pub start_col: usize,
    /// One-based ending line, when known.
    pub end_line: Option<usize>,
    /// Zero-based exclusive ending column, when known.
    pub end_col: Option<usize>,
}

/// A source line and its optional highlighted range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSourceLine {
    /// One-based line number.
    pub line_num: usize,
    /// Zero-based Unicode scalar column where the retained text begins.
    pub start_column: usize,
    /// Zero-based UTF-16 column where the retained text begins.
    pub start_character: usize,
    /// Whether the retained text reaches the original source line's end.
    pub ends_line: bool,
    /// Source text.
    pub text: String,
    /// Zero-based, end-exclusive highlight range.
    pub highlight: Option<(usize, usize)>,
}

impl From<&SourceLine> for ResolvedSourceLine {
    fn from(value: &SourceLine) -> Self {
        Self {
            line_num: value.line_num,
            start_column: value.start_column,
            start_character: value.start_character,
            ends_line: value.ends_line,
            text: value.text.clone(),
            highlight: value.highlight,
        }
    }
}

/// Stable, serializable tracepoint classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum ResolvedTraceKind {
    /// Function call, optionally named.
    Call(Option<String>),
    /// Show-rule application.
    Show(String),
    /// Module import.
    Import(String),
    /// Module include.
    Include(String),
}

impl From<&TraceKind> for ResolvedTraceKind {
    fn from(value: &TraceKind) -> Self {
        match value {
            TraceKind::Call(name) => Self::Call(name.clone()),
            TraceKind::Show(name) => Self::Show(name.clone()),
            TraceKind::Import(name) => Self::Import(name.clone()),
            TraceKind::Include(name) => Self::Include(name.clone()),
        }
    }
}

/// A resolved hint, retaining detached/spanned location information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedHint {
    /// Hint text.
    pub message: String,
    /// Hint source location, when available.
    pub source: Option<ResolvedSource>,
    /// Bounded source context at the hint location.
    pub source_lines: Vec<ResolvedSourceLine>,
    /// Why location resolution failed.
    pub location_failure: Option<ResolvedLocationFailure>,
    /// Source-context truncation metadata.
    pub truncation: ResolvedTruncation,
}

impl From<&Hint> for ResolvedHint {
    fn from(value: &Hint) -> Self {
        let location = &value.location;
        Self {
            message: value.message.clone(),
            source: location.path.clone().map(|path| ResolvedSource {
                path,
                line: location.line,
                column: location.column,
                range: location.range,
            }),
            source_lines: location
                .source_lines
                .iter()
                .map(ResolvedSourceLine::from)
                .collect(),
            location_failure: location.location_failure.map(ResolvedLocationFailure::from),
            truncation: location.truncation.into(),
        }
    }
}

/// One resolved trace entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTrace {
    /// Structured tracepoint kind.
    pub kind: ResolvedTraceKind,
    /// Human-readable trace label/message.
    pub label: String,
    /// Source location, when available.
    pub source: Option<ResolvedSource>,
    /// Full trace message.
    pub message: String,
    /// Resolved source lines at the trace point.
    pub source_lines: Vec<ResolvedSourceLine>,
    /// Why location resolution failed.
    pub location_failure: Option<ResolvedLocationFailure>,
    /// Source-context truncation metadata.
    pub truncation: ResolvedTruncation,
}

impl From<&Trace> for ResolvedTrace {
    fn from(value: &Trace) -> Self {
        let location = &value.location;
        Self {
            kind: ResolvedTraceKind::from(&value.kind),
            label: value.message.clone(),
            source: location.path.clone().map(|path| ResolvedSource {
                path,
                line: location.line,
                column: location.column,
                range: location.range,
            }),
            message: value.message.clone(),
            source_lines: location
                .source_lines
                .iter()
                .map(ResolvedSourceLine::from)
                .collect(),
            location_failure: location.location_failure.map(ResolvedLocationFailure::from),
            truncation: location.truncation.into(),
        }
    }
}

/// A resolved diagnostic suitable for JSON, WebSocket, or other API boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDiagnostic {
    /// The producer identity and evidence retained by the native diagnostic.
    pub origin: crate::diagnostic::DiagnosticOrigin,
    /// Stable caller-visible identifier within a diagnostics collection.
    pub id: String,
    /// Diagnostic severity.
    pub severity: ResolvedSeverity,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Primary source location, when available.
    pub source: Option<ResolvedSource>,
    /// Primary resolved span, when available.
    pub span: Option<ResolvedSpan>,
    /// Source lines covered by the primary span.
    pub source_lines: Vec<ResolvedSourceLine>,
    /// Trace entries.
    pub trace: Vec<ResolvedTrace>,
    /// Static package navigation hints; see [`super::super::Diagnostic::imported_by`].
    pub imported_by: Vec<String>,
    /// The package this diagnostic imports that could not be provided.
    pub package_failure: Option<ResolvedPackageFailure>,
    /// Suggested fixes or hints, including optional secondary locations.
    pub hints: Vec<ResolvedHint>,
    /// Additional notes (reserved for producers that provide them).
    pub notes: Vec<String>,
    /// Why the primary source location could not be resolved.
    pub location_failure: Option<ResolvedLocationFailure>,
    /// Source-context truncation metadata.
    pub truncation: ResolvedTruncation,
}

impl ResolvedDiagnostic {
    fn content_id(&self) -> String {
        let mut clone = self.clone();
        clone.id.clear();
        let bytes = serde_json::to_vec(&clone).expect("resolved diagnostic is serializable");
        blake3::hash(&bytes).to_hex().to_string()
    }
}

impl Diagnostic {
    /// Convert to a resolved diagnostic with an empty identifier.
    pub fn to_resolved(&self) -> ResolvedDiagnostic {
        let mut resolved = self.to_resolved_with_id(String::new());
        resolved.id = resolved.content_id();
        resolved
    }

    /// Convert to a resolved diagnostic with an explicit stable identifier.
    pub fn to_resolved_with_id(&self, id: impl Into<String>) -> ResolvedDiagnostic {
        let location = &self.location;
        let lines = location.source_lines.as_slice();
        let span = lines.first().and_then(|first| {
            first.highlight.map(|(start, _)| {
                let (end_line, end_col) = lines
                    .last()
                    .and_then(|last| {
                        last.highlight
                            .map(|(_, end)| (Some(last.line_num), Some(last.start_column + end)))
                    })
                    .unwrap_or((None, None));
                ResolvedSpan {
                    start_line: first.line_num,
                    start_col: first.start_column + start,
                    end_line,
                    end_col,
                }
            })
        });
        ResolvedDiagnostic {
            origin: self.origin.clone(),
            id: id.into(),
            severity: self.severity.into(),
            message: self.message.clone(),
            source: location.path.clone().map(|path| ResolvedSource {
                path,
                line: location.line,
                column: location.column,
                range: location.range,
            }),
            span,
            source_lines: lines.iter().map(ResolvedSourceLine::from).collect(),
            trace: self.traces.iter().map(ResolvedTrace::from).collect(),
            imported_by: self.imported_by.clone(),
            package_failure: self.package_failure.clone(),
            hints: self.hints.iter().map(ResolvedHint::from).collect(),
            notes: Vec::new(),
            location_failure: location.location_failure.map(ResolvedLocationFailure::from),
            truncation: location.truncation.into(),
        }
    }
}

impl Diagnostics {
    /// Convert all diagnostics to serializable resolved representations.
    pub fn to_resolved_diagnostics(&self) -> Vec<ResolvedDiagnostic> {
        self.iter().map(Diagnostic::to_resolved).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::message::{SourceLocation, SourceRange};
    use typst::diag::Severity;

    #[test]
    fn content_id_changes_with_message() {
        let info = Diagnostic {
            origin: crate::diagnostic::DiagnosticOrigin::Typst,
            severity: Severity::Error,
            message: "bad input".into(),
            location: SourceLocation {
                path: Some("main.typ".into()),
                line: Some(3),
                column: Some(4),
                range: None,
                source_lines: vec![],
                location_failure: None,
                truncation: SourceTruncation::default(),
            },
            hints: vec![],
            traces: vec![],
            imported_by: vec![],
            package_failure: None,
        };
        let first = info.to_resolved();
        let second = info.to_resolved();
        assert_eq!(first.id, second.id);

        let mut changed = info.clone();
        changed.message.push('!');
        assert_ne!(first.id, changed.to_resolved().id);
    }

    #[test]
    fn resolved_diagnostic_serde_roundtrips() {
        let info = Diagnostic {
            origin: crate::diagnostic::DiagnosticOrigin::Typst,
            severity: Severity::Error,
            message: "bad input".into(),
            location: SourceLocation {
                path: Some("main.typ".into()),
                line: Some(3),
                column: Some(4),
                range: Some(SourceRange {
                    start: crate::diagnostic::message::SourcePosition {
                        line: 2,
                        character: 3,
                    },
                    end: crate::diagnostic::message::SourcePosition {
                        line: 4,
                        character: 7,
                    },
                }),
                source_lines: vec![SourceLine {
                    line_num: 3,
                    start_column: 0,
                    start_character: 0,
                    ends_line: true,
                    text: "#bad".into(),
                    highlight: Some((1, 4)),
                }],
                location_failure: None,
                truncation: SourceTruncation::default(),
            },
            hints: vec![Hint {
                message: "fix it".into(),
                location: SourceLocation {
                    path: None,
                    line: None,
                    column: None,
                    range: None,
                    source_lines: vec![],
                    location_failure: Some(LocationFailure::DetachedSpan),
                    truncation: SourceTruncation::default(),
                },
            }],
            traces: vec![],
            imported_by: vec![],
            package_failure: None,
        };
        let resolved = info.to_resolved_with_id("diag-1");
        let json = serde_json::to_string(&resolved).unwrap();
        let decoded: ResolvedDiagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, resolved);
        assert_eq!(decoded.severity, ResolvedSeverity::Error);
        assert_eq!(decoded.id, "diag-1");
    }
}
