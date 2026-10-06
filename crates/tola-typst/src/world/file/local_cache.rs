//! Parsed values that live for one task and are never shared.

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::Loaded;

pub(crate) struct LocalFileCache {
    pub(crate) sources: RwLock<FxHashMap<FileId, Loaded<Source>>>,
    pub(crate) files: RwLock<FxHashMap<FileId, Loaded<Bytes>>>,
}

impl LocalFileCache {
    pub(crate) fn new() -> Self {
        Self {
            sources: RwLock::new(FxHashMap::default()),
            files: RwLock::new(FxHashMap::default()),
        }
    }

    /// Drop all task-local file values before a subsequent compilation.
    pub(crate) fn reset(&self) {
        self.sources.write().clear();
        self.files.write().clear();
    }
}

impl Default for LocalFileCache {
    fn default() -> Self {
        Self::new()
    }
}
