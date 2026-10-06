//! Resolved diagnostics: what a live diagnostic has, and the retired DTOs.
//!
//! [`ResolvedSource`], [`ResolvedPackageFailure`], and their parts are read by live
//! diagnostics and by [`resolve_source`](super::source::resolve_source).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::diagnostic::message::SourceRange;

#[cfg(feature = "legacy-serialization")]
#[deprecated(note = "native Typst values pass through the pipeline; scheduled for deletion")]
mod serialized;
#[cfg(feature = "legacy-serialization")]
#[allow(deprecated)]
pub use serialized::{
    ResolvedDiagnostic, ResolvedHint, ResolvedLocationFailure, ResolvedSeverity,
    ResolvedSourceLine, ResolvedSpan, ResolvedTrace, ResolvedTraceKind, ResolvedTruncation,
};

/// A source location attached to a diagnostic or trace entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSource {
    /// Virtual source path or path relative to the compilation root.
    pub path: String,
    /// One-based line number.
    pub line: Option<usize>,
    /// One-based column number.
    pub column: Option<usize>,
    /// Exact zero-based UTF-16 range, independent of display source excerpts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<SourceRange>,
}

/// One package directory checked while looking for a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPackageSearch {
    /// The package root that was searched.
    pub root: PathBuf,
    /// The package directory expected inside that root.
    pub candidate: PathBuf,
}

/// Why a package could not be provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedPackageFailureReason {
    /// No checked directory contained the package.
    NotInstalled,
    /// The package was not available locally and could not be installed.
    Unavailable,
}

/// A package a diagnostic imports that could not be provided.
///
/// Typst reports the failure at the import statement without naming a package
/// directory, so this has the identity the author wrote, why it stopped, and
/// every directory that was checked, in search order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPackageFailure {
    /// Package identity, written `@namespace/name:version`.
    pub package: String,
    /// Why the package could not be provided.
    pub reason: ResolvedPackageFailureReason,
    /// Package directories that were checked, in search order.
    pub searched: Vec<ResolvedPackageSearch>,
}
