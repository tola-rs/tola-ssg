//! Internal execution modes for `TypstWorld`.

use std::sync::Arc;

use typst::Library;
use typst::utils::LazyHash;

use super::snapshot::SourceSnapshot;
use crate::world::file::{FileResolver, FileSnapshot, SharedFileCache};
use crate::world::font::FontStore;
use crate::world::library::create_library_with_inputs;

pub(crate) enum FileCacheMode {
    /// Task-local cache, no sharing between tasks.
    Local,
    /// Shared cache owned by the caller.
    Shared(Arc<SharedFileCache>),
    /// Pre-built source snapshot plus a shared fallback cache.
    Snapshot {
        /// Immutable snapshot containing preloaded root sources.
        snapshot: Arc<SourceSnapshot>,
        /// Shared cache used for files that are not covered by the snapshot.
        fallback: Arc<SharedFileCache>,
    },
    /// An externally owned frozen file view.
    Frozen {
        files: Arc<FileSnapshot>,
        parsed: Arc<SharedFileCache>,
    },
}

impl FileCacheMode {
    pub(crate) fn local() -> Self {
        Self::Local
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

    pub(crate) fn frozen(files: Arc<FileSnapshot>, parsed: Arc<SharedFileCache>) -> Self {
        Self::Frozen { files, parsed }
    }
}

pub(crate) enum FileAccess {
    Local(Arc<FileSnapshot>),
    Live {
        files: Arc<FileResolver>,
        sources: Option<Arc<SourceSnapshot>>,
    },
    Frozen(Arc<FileSnapshot>),
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
