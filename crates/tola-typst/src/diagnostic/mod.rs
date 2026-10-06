//! Diagnostic values for Typst compilation errors and warnings.

mod collection;
mod error;
mod filter;
mod message;
mod native;
pub(crate) mod package_imports;
mod panic;
mod resolved;
mod source;

pub use collection::{DiagnosticSummary, Diagnostics};
pub use error::CompileError;
pub(crate) use error::{CancelledError, export_error_message};
pub use filter::{DiagnosticFilter, FilterType, PackageKind};
pub use message::{
    Diagnostic, Hint, LocationFailure, SourceContextLimit, SourceLine, SourceLocation,
    SourcePosition, SourceRange, SourceTruncation, Trace, TraceKind,
};
pub use native::{DiagnosticOrigin, NativeDiagnostic, ProducerDiagnosticOrigin, producer_warning};
pub use panic::{ExplainedPanic, explained_panic};
#[cfg(feature = "legacy-serialization")]
#[allow(deprecated)]
pub use resolved::{
    ResolvedDiagnostic, ResolvedHint, ResolvedLocationFailure, ResolvedSeverity,
    ResolvedSourceLine, ResolvedSpan, ResolvedTrace, ResolvedTraceKind, ResolvedTruncation,
};
pub use resolved::{
    ResolvedPackageFailure, ResolvedPackageFailureReason, ResolvedPackageSearch, ResolvedSource,
};
pub use source::{
    resolve_diagnostic, resolve_diagnostic_with_options, resolve_source, resolve_source_location,
};

pub use typst::diag::{Severity as DiagnosticSeverity, SourceDiagnostic};
