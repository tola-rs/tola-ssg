//! Pure file target resolution for Typst file IDs.

use std::path::PathBuf;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use typst::syntax::FileId;

/// A provider-selected source for one Typst file.
///
/// A provider resolves identities only. Observing the filesystem inside `target` would escape the
/// resolver's snapshot and its dependency evidence; a provider that needs content returns
/// [`FileTarget::Disk`] and lets the resolver read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTarget {
    /// Immutable bytes supplied without a filesystem read.
    Bytes(Arc<[u8]>),
    /// An absolute path which the resolver must read itself.
    ///
    /// Relative paths are rejected; a provider must resolve them against its
    /// own root before returning this target.
    Disk(PathBuf),
    /// The provider owns this identity, but no file exists in its immutable view.
    /// This stops resolution without falling back to root files or packages.
    Missing,
}

/// Pure mapping from a Typst file identity to bytes, a disk target, or an owned absence.
pub trait FileProvider: Send + Sync {
    /// Resolve a file without touching the filesystem.
    fn target(&self, id: FileId) -> Option<FileTarget>;

    /// Package namespaces this provider is the only source for.
    ///
    /// An identity in one of them never falls back to a package directory, so the files the
    /// provider serves are the whole namespace: a directory holding a package in it is ignored
    /// rather than shadowing or extending the provider's set.
    fn owned_namespaces(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Provider which never overrides normal root or package resolution.
#[derive(Debug, Default)]
pub struct EmptyFiles;

impl FileProvider for EmptyFiles {
    fn target(&self, _id: FileId) -> Option<FileTarget> {
        None
    }
}

/// In-memory files keyed by their complete Typst identity.
#[derive(Clone, Debug, Default)]
pub struct FileMap {
    files: FxHashMap<FileId, Arc<[u8]>>,
}

impl FileMap {
    /// Create an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace one file.
    pub fn insert(&mut self, id: FileId, content: impl Into<Arc<[u8]>>) {
        self.files.insert(id, content.into());
    }

    /// Insert a root file by physical path and return its virtual identity.
    ///
    /// Returns None when the path is outside root or cannot be represented
    /// as a Typst virtual path.
    pub fn insert_path<P, R>(
        &mut self,
        path: P,
        root: R,
        content: impl Into<Arc<[u8]>>,
    ) -> Option<FileId>
    where
        P: AsRef<std::path::Path>,
        R: AsRef<std::path::Path>,
    {
        let id = crate::world::file::file_id_from_path(path.as_ref(), root.as_ref())?;
        self.insert(id, content);
        Some(id)
    }

    /// Remove one file.
    pub fn remove(&mut self, id: FileId) -> Option<Arc<[u8]>> {
        self.files.remove(&id)
    }

    /// Return whether the exact file identity is present.
    pub fn contains(&self, id: FileId) -> bool {
        self.files.contains_key(&id)
    }

    /// Return the number of files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Return whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl FileProvider for FileMap {
    fn target(&self, id: FileId) -> Option<FileTarget> {
        self.files.get(&id).cloned().map(FileTarget::Bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

    #[test]
    fn file_map_keys_on_full_identity() {
        let root = std::path::Path::new("/workspace");
        let root_file = crate::world::file::file_id("lib.typ");
        let package = FileId::new(RootedPath::new(
            VirtualRoot::Package("@preview/example:1.0.0".parse().unwrap()),
            VirtualPath::new("lib.typ").unwrap(),
        ));
        let mut files = FileMap::new();
        files.insert(root_file, Arc::<[u8]>::from(&b"root"[..]));

        assert!(matches!(
            files.target(root_file),
            Some(FileTarget::Bytes(_))
        ));
        assert_eq!(files.target(package), None);

        assert_eq!(
            files.insert_path(std::path::Path::new("/other/file.typ"), root, b"x".to_vec()),
            None
        );
        assert_eq!(files.len(), 1);
    }
}
