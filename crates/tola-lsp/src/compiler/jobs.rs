//! The requests one source job hands to the compiler lane.

use std::path::PathBuf;
use std::sync::Arc;

use lsp_server::RequestId;
use lsp_types::Uri;
use tola_build::cancellation::BuildCancellation;

use crate::protocol::{CheckProgress, SourceQuery};

/// One unsaved source: the site path it belongs to, and the text the editor holds.
pub(crate) type UnsavedSource = (PathBuf, Arc<str>);

pub(crate) type SourceOverrides = Arc<[UnsavedSource]>;

pub(crate) struct SourceInputs {
    pub(crate) root: PathBuf,
    pub(crate) overrides: SourceOverrides,
    /// The source revision these unsaved sources belong to, which is what a compilation may be
    /// reused within.
    pub(crate) source_revision: u64,
    pub(crate) cancellation: BuildCancellation,
}

pub(crate) struct CheckRequest {
    /// The revision this check answers, which the connection compares to discard a check whose
    /// sources a newer change already superseded.
    pub(crate) checked_revision: u64,
    pub(crate) sources: SourceInputs,
    /// The open documents of this revision, which the check's own selection index reuses and its
    /// liveness pass licenses.
    pub(crate) view: crate::sources::SourceView,
}

pub(crate) struct QueryRequest {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
    pub(crate) view: crate::sources::SourceView,
    pub(crate) package_sources: Option<Arc<PathBuf>>,
    pub(crate) routes_as_hints: bool,
    pub(crate) check_progress: CheckProgress,
    pub(crate) query: SourceQuery,
}

pub(crate) struct AnalysisRequest {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) root: PathBuf,
    pub(crate) view: crate::sources::SourceView,
    pub(crate) source: tola_typst::typst::syntax::Source,
    pub(crate) cursor: usize,
    pub(crate) package_locations: Option<tola_typst::PackageLocations>,
    pub(crate) boundary: tola_typst::SourceBoundary,
    pub(crate) query: SourceQuery,
    pub(crate) cancellation: BuildCancellation,
    /// The source revision this request's graph belongs to, which its reuse must match.
    pub(crate) source_revision: u64,
}

/// The request one source-only analysis hands to the compiler lane.
///
/// The analysis proved an import identity the query needs — the same id, serial, view, and
/// cancellation cross over, so the compiler lane answers the client's request exactly once.
pub(crate) struct AnalysisContinuation {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) root: PathBuf,
    pub(crate) view: crate::sources::SourceView,
    pub(crate) query: SourceQuery,
    pub(crate) cancellation: BuildCancellation,
}

pub(crate) enum SourceJob {
    Check(CheckRequest),
    Query(QueryRequest),
    Route(RouteRead),
    Lenses(LensesRead),
    Rename(RenameRead),
    Symbols(SymbolsRead),
    PackageSource(PackageSourceRead),
    IncomingCalls(IncomingCallsRead),
    /// Index every page the site realizes, which no source's own request names.
    RouteIndex(RouteIndexRead),
    /// Build the site's selection index for the revision a correction reads, which no check that
    /// revision has completed could return.
    Selection(SelectionRead),
    /// Let go of the compilation this lane retains, which nothing has read for a while.
    ReleaseIdle,
}

impl SourceJob {
    pub(crate) fn is_cancelled(&self) -> bool {
        match self {
            Self::Check(request) => request.sources.cancellation.is_cancelled(),
            Self::Query(request) => request.sources.cancellation.is_cancelled(),
            Self::Route(request) => request.sources.cancellation.is_cancelled(),
            Self::Lenses(request) => request.sources.cancellation.is_cancelled(),
            Self::Rename(request) => request.sources.cancellation.is_cancelled(),
            Self::Symbols(request) => request.sources.cancellation.is_cancelled(),
            Self::PackageSource(request) => request.cancellation.is_cancelled(),
            Self::RouteIndex(request) => request.sources.cancellation.is_cancelled(),
            Self::IncomingCalls(request) => request.sources.cancellation.is_cancelled(),
            Self::Selection(request) => request.cancellation.is_cancelled(),
            Self::ReleaseIdle => false,
        }
    }
}

/// One `tola/route` request, answered from a check of the site's sources.
pub(crate) struct RouteRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
    pub(crate) uri: Uri,
}

/// One `tola/routes` request: every page the site realizes, which no document names.
pub(crate) struct RouteIndexRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
}

/// One selection index a correction reads: the revision its answer belongs to, and the site whose
/// sources the index judges.
///
/// The root travels with the request, because it is the root the connection keys the answer under:
/// the resolved configuration's own root, or the workspace root when no check has resolved one.
pub(crate) struct SelectionRead {
    pub(crate) revision: u64,
    pub(crate) root: PathBuf,
    pub(crate) view: crate::sources::SourceView,
    pub(crate) cancellation: BuildCancellation,
}

pub(crate) struct LensesRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
    pub(crate) uri: Uri,
}

pub(crate) struct RenameRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
    /// The files the editor renames: where each was, and where it goes.
    pub(crate) files: Vec<(PathBuf, PathBuf)>,
}

pub(crate) struct SymbolsRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
    pub(crate) query: String,
}

pub(crate) struct PackageSourceRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) uri: Uri,
    pub(crate) cancellation: BuildCancellation,
}

/// One `callHierarchy/incomingCalls` request, answered from the sources that reach the item's file.
pub(crate) struct IncomingCallsRead {
    pub(crate) id: RequestId,
    pub(crate) serial: u64,
    pub(crate) sources: SourceInputs,
    pub(crate) view: crate::sources::SourceView,
    pub(crate) source: tola_typst::typst::syntax::Source,
}
