//! Internal execution modes for `TypstWorld`.

use std::sync::Arc;

use typst::Library;
use typst::utils::LazyHash;

use super::sources::ExplicitSources;
use crate::world::file::{FileStore, FileView, SharedFileCache};
use crate::world::font::FontStore;
use crate::world::library::create_library_with_inputs;

pub(crate) enum FileCacheMode {
    Local,
    Shared(Arc<SharedFileCache>),
    Sources {
        sources: Arc<ExplicitSources>,
        fallback: Arc<SharedFileCache>,
    },
    Frozen {
        files: Arc<FileView>,
        parsed: Arc<SharedFileCache>,
    },
}

pub(crate) enum FileAccess {
    Local(Arc<FileView>),
    Live {
        files: Arc<FileStore>,
        sources: Option<Arc<ExplicitSources>>,
    },
    Frozen(Arc<FileView>),
}

#[derive(Clone)]
pub(crate) enum FontMode {
    None,
    Shared(Arc<FontStore>),
}

#[derive(Clone)]
pub(crate) enum LibraryMode {
    Global,
    Custom(Arc<LazyHash<Library>>),
}

impl LibraryMode {
    pub(crate) fn custom(inputs: typst::foundations::Dict) -> Self {
        Self::Custom(Arc::new(create_library_with_inputs(inputs)))
    }
}
