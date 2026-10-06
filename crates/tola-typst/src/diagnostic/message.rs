//! Resolved diagnostic messages, hints, traces, and bounded source context.

use serde::{Deserialize, Serialize};
use typst::diag::Severity;

/// Why a diagnostic source location could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationFailure {
    /// The span is detached from any source file.
    DetachedSpan,
    /// The source file could not be loaded from the world.
    SourceUnavailable,
    /// Typst did not provide a byte range for the span.
    RangeUnavailable,
    /// The range starts after it ends.
    ReversedRange,
    /// The range exceeds the source text bounds.
    OutOfBounds,
    /// A range endpoint is not a UTF-8 character boundary.
    InvalidUtf8Boundary,
}

/// Limits source context retained while resolving diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceContextLimit {
    /// Maximum distinct source lines retained per location. Zero retains no excerpt.
    pub max_lines: usize,
    /// Maximum bytes retained per source line, shared by any disjoint windows.
    pub max_line_bytes: usize,
}

impl SourceContextLimit {
    /// Retain no excerpt, for a caller that needs only the path and position of a span.
    pub(crate) const POSITION_ONLY: Self = Self {
        max_lines: 0,
        max_line_bytes: 0,
    };
}

impl Default for SourceContextLimit {
    fn default() -> Self {
        Self {
            max_lines: 32,
            max_line_bytes: 4096,
        }
    }
}

/// Truncation applied to resolved source context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceTruncation {
    /// Number of omitted source lines.
    pub omitted_lines: usize,
    /// Number of omitted bytes.
    pub omitted_bytes: usize,
}

/// A zero-based source position with a UTF-16 code-unit column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePosition {
    /// Zero-based line number.
    pub line: usize,
    /// Zero-based UTF-16 code-unit offset within the line.
    pub character: usize,
}

/// An exact, end-exclusive source range independent of display context limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    /// Inclusive start position.
    pub start: SourcePosition,
    /// Exclusive end position.
    pub end: SourcePosition,
}

/// One source location: where a diagnostic, hint, or trace sits, and what was retained of it.
///
/// The path and position come from the same immutable source revision as the excerpt, so a
/// renderer never has to re-resolve or re-read the file to point at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// File path (if available).
    pub path: Option<String>,
    /// Line number (1-indexed, if available).
    pub line: Option<usize>,
    /// One-based Unicode scalar column, if available.
    pub column: Option<usize>,
    /// Exact UTF-16 range resolved from the compilation's source revision.
    pub range: Option<SourceRange>,
    /// Source code lines with highlighting info.
    pub source_lines: Vec<SourceLine>,
    /// Why the location could not be resolved.
    pub location_failure: Option<LocationFailure>,
    /// Source context truncation metadata.
    pub truncation: SourceTruncation,
}

impl SourceLocation {
    /// A location whose span has no source file this world could read.
    pub(crate) fn unresolved(failure: LocationFailure) -> Self {
        Self {
            path: None,
            line: None,
            column: None,
            range: None,
            source_lines: Vec::new(),
            location_failure: Some(failure),
            truncation: SourceTruncation::default(),
        }
    }

    /// The location a reader can search for: `path:line:column`, `path:line`, or `path`.
    ///
    /// A renderer that cannot read the file still names what the producer knew.
    pub(crate) fn spelled(&self) -> Option<String> {
        let path = self.path.as_deref()?;
        Some(match (self.line, self.column) {
            (Some(line), Some(column)) => format!("{path}:{line}:{column}"),
            (Some(line), None) => format!("{path}:{line}"),
            _ => path.to_owned(),
        })
    }
}

/// Structured diagnostic information for custom rendering.
///
/// Includes a resolved location, bounded source context, hints, and traces.
///
/// # Example
///
/// ```ignore
/// use tola_typst::diagnostic::{Diagnostic, resolve_diagnostic};
///
/// let info = resolve_diagnostic(&world, &diag);
///
/// println!(
///     "Error at {}:{}",
///     info.location.path.as_deref().unwrap_or("?"),
///     info.location.line.unwrap_or(0)
/// );
/// for line in &info.location.source_lines {
///     println!("{}: {}", line.line_num, line.text);
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The native producer identity and evidence, retained independently of presentation.
    pub origin: super::DiagnosticOrigin,
    /// Error severity (error or warning).
    pub severity: Severity,
    /// The error message.
    pub message: String,
    /// Where the diagnostic points.
    pub location: SourceLocation,
    /// Structured hints, including optional secondary locations.
    pub hints: Vec<Hint>,
    /// Stack trace entries.
    pub traces: Vec<Trace>,
    /// Site files containing literal imports or includes of the package named by the location.
    ///
    /// Added when a package diagnostic has no trace. These are static navigation hints, not
    /// proof that the statements executed.
    pub imported_by: Vec<String>,
    /// The package this diagnostic imports that could not be provided.
    pub package_failure: Option<super::resolved::ResolvedPackageFailure>,
}

/// A retained window of one source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLine {
    /// Line number (1-indexed).
    pub line_num: usize,
    /// Zero-based Unicode scalar column where the retained text begins.
    pub start_column: usize,
    /// Zero-based UTF-16 column where the retained text begins.
    pub start_character: usize,
    /// Whether the retained text reaches the original source line's end.
    pub ends_line: bool,
    /// The source text.
    pub text: String,
    /// Zero-based Unicode scalar highlight offsets within the retained text.
    pub highlight: Option<(usize, usize)>,
}

/// A resolved diagnostic hint with an optional secondary location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// Hint text.
    pub message: String,
    /// Where the hint points. A detached hint retains `DetachedSpan`.
    pub location: SourceLocation,
}

/// Stable classification of a Typst tracepoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceKind {
    /// Function call, optionally named.
    Call(Option<String>),
    /// Show-rule application.
    Show(String),
    /// Module import.
    Import(String),
    /// Module include.
    Include(String),
}

/// Stack trace entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    /// Structured tracepoint kind.
    pub kind: TraceKind,
    /// Description of the trace point (from Typst's Tracepoint::Display).
    pub message: String,
    /// Where the trace point sits in the source.
    pub location: SourceLocation,
}
