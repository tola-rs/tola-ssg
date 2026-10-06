//! Internal execution modes for `TypstWorld`.

use std::sync::Arc;

use typst::Library;
use typst::utils::LazyHash;

use super::file::LocalFileCache;
use super::snapshot::SourceSnapshot;
use crate::world::file::{CandidateFileSnapshot, SharedFileCache};
use crate::world::font::FontStore;
use crate::world::library::create_library_with_inputs;

pub(crate) enum FileCacheMode {
    /// Task-local cache, no sharing between tasks.
    Local(LocalFileCache),
    /// Shared cache owned by the caller.
    Shared(Arc<SharedFileCache>),
    /// Pre-built source snapshot plus a shared fallback cache.
    Snapshot {
        /// Immutable snapshot containing preloaded root sources.
        snapshot: Arc<SourceSnapshot>,
        /// Shared cache used for files that are not covered by the snapshot.
        fallback: Arc<SharedFileCache>,
    },
    /// One candidate-scoped view of explicit and first-observed files.
    Candidate {
        files: Arc<CandidateFileSnapshot>,
        parsed: Arc<SharedFileCache>,
    },
}

impl FileCacheMode {
    pub(crate) fn local() -> Self {
        Self::Local(LocalFileCache::new())
    }

    pub(crate) fn shared(cache: Arc<SharedFileCache>) -> Self {
        Self::Shared(cache)
    }

    pub(crate) fn snapshot(snapshot: Arc<SourceSnapshot>) -> Self {
        Self::Snapshot {
            snapshot,
            fallback: Arc::new(SharedFileCache::new()),
        }
    }

    pub(crate) fn candidate(
        files: Arc<CandidateFileSnapshot>,
        parsed: Arc<SharedFileCache>,
    ) -> Self {
        Self::Candidate { files, parsed }
    }
}

#[derive(Clone)]
pub(crate) enum FontMode {
    /// No fonts loaded (for scan/query).
    None,
    /// Shared fonts owned by the caller.
    Shared(Arc<FontStore>),
}

/// Library mode for `sys.inputs`.
#[derive(Clone)]
pub(crate) enum LibraryMode {
    /// Use global library (no sys.inputs).
    Global,
    /// Custom library with sys.inputs.
    Custom(Arc<LazyHash<Library>>),
}

impl LibraryMode {
    pub(crate) fn custom(inputs: typst::foundations::Dict) -> Self {
        Self::Custom(Arc::new(create_library_with_inputs(inputs)))
    }
}
