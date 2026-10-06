//! Typst world with configurable file caches, fonts, and library inputs.
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use tola_typst::{BundleCancellation, FontStore, SharedFileCache, TypstWorld};
//!
//! let cancellation = BundleCancellation::default();
//!
//! // Scan: no fonts, local cache
//! let world = TypstWorld::builder(path, root)
//!     .with_local_cache()
//!     .no_fonts()
//!     .build(&cancellation);
//!
//! // Build: with fonts, snapshot cache
//! let world = TypstWorld::builder(path, root)
//!     .with_snapshot(snapshot)
//!     .with_fonts(Arc::new(FontStore::new()))
//!     .build(&cancellation);
//!
//! // Serve: with fonts, shared cache
//! let world = TypstWorld::builder(path, root)
//!     .with_shared_cache(Arc::new(SharedFileCache::new()))
//!     .with_fonts(Arc::new(FontStore::new()))
//!     .build(&cancellation);
//!
//! // With sys.inputs
//! let world = TypstWorld::builder(path, root)
//!     .with_local_cache()
//!     .no_fonts()
//!     .with_inputs([("key", "value")])
//!     .build(&cancellation);
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, World};

use super::builder::{WorldBuildError, WorldBuilder};
use super::path::normalize_path;
use super::strategy::{FileCacheMode, FontMode, LibraryMode};
use crate::world::file::{FileResolver, Loaded, ReadAttempt, decode_utf8, file_id_from_path};
use crate::world::library::GLOBAL_LIBRARY;

static EMPTY_FONTBOOK: OnceLock<LazyHash<FontBook>> = OnceLock::new();

fn empty_fontbook() -> &'static LazyHash<FontBook> {
    EMPTY_FONTBOOK.get_or_init(|| LazyHash::new(FontBook::new()))
}

/// Typst world configured through [`Self::builder`].
pub struct TypstWorld {
    root: PathBuf,
    main: FileId,
    files: Arc<FileResolver>,
    cache: FileCacheMode,
    fonts: FontMode,
    library: LibraryMode,
    time: Option<typst_kit::datetime::Time>,
}

impl TypstWorld {
    /// Select the file cache and font policies this world compiles with.
    pub fn builder(main_path: &Path, root: &Path) -> WorldBuilder {
        WorldBuilder::new(main_path, root)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        main_path: &Path,
        root: &Path,
        files: Arc<FileResolver>,
        cache: FileCacheMode,
        fonts: FontMode,
        library: LibraryMode,
        time: Option<typst_kit::datetime::Time>,
    ) -> Result<Self, WorldBuildError> {
        files
            .source_boundary()
            .check(main_path)
            .map_err(WorldBuildError::Source)?;
        let root = normalize_path(root);
        let main_abs = normalize_path(main_path);
        let main = file_id_from_path(&main_abs, &root).ok_or_else(|| {
            WorldBuildError::MainOutsideRoot {
                main: main_abs.clone(),
                root: root.clone(),
            }
        })?;
        let snapshot_root = match &cache {
            FileCacheMode::Snapshot { snapshot, .. } => Some(snapshot.root()),
            FileCacheMode::Candidate { files, .. } => Some(files.root()),
            FileCacheMode::Local(_) | FileCacheMode::Shared(_) => None,
        };
        if let Some(snapshot_root) = snapshot_root
            && snapshot_root != root
        {
            return Err(WorldBuildError::SnapshotRootMismatch {
                snapshot: snapshot_root.to_path_buf(),
                world: root,
            });
        }
        if let FileCacheMode::Candidate {
            files: candidate, ..
        } = &cache
            && candidate.source_boundary() != files.source_boundary()
        {
            return Err(WorldBuildError::SnapshotBoundaryMismatch);
        }

        Ok(Self {
            root,
            main,
            files,
            cache,
            fonts,
            library,
            time,
        })
    }

    pub(crate) fn font_failure(&self) -> Option<crate::world::font::FontLoadError> {
        match &self.fonts {
            FontMode::None => None,
            FontMode::Shared(fonts) => fonts.read_failure(),
        }
    }

    /// Normalized root that every file identity is resolved against.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read bytes through this world's file view and retain their dependency evidence.
    ///
    /// Consumers extending a compiled result with derived resources must use the
    /// same snapshot and hold these inputs to their publication checks. Failures
    /// retain attempted paths and package-selection evidence in the second value.
    pub fn read_file_with_evidence(
        &self,
        id: FileId,
    ) -> (FileResult<Bytes>, crate::session::AccessedDeps) {
        let ReadAttempt {
            result,
            reads,
            disk_reads,
            package_checks,
        } = self.get_file(id);
        (
            result.map(|loaded| loaded.value),
            crate::session::AccessedDeps {
                reads: reads.into_iter().collect(),
                disk_reads,
                package_checks,
            },
        )
    }

    /// Reset per-world inputs before another compilation.
    ///
    /// Shared caches revalidate on access; immutable snapshots must be rebuilt
    /// by the caller when source membership changes. Resets system time but
    /// leaves fixed datetimes unchanged.
    pub fn reset(&mut self) {
        if let FileCacheMode::Local(local) = &self.cache {
            local.reset();
        }
        if let Some(time) = &mut self.time {
            time.reset();
        }
    }

    pub(crate) fn get_source(&self, id: FileId) -> ReadAttempt<Loaded<Source>> {
        match &self.cache {
            FileCacheMode::Local(local) => {
                if let Some(source) = local.sources.read().get(&id) {
                    return ReadAttempt::from_loaded(source.clone());
                }
                let attempt = self.load_source(id);
                if let Ok(source) = &attempt.result {
                    local.sources.write().insert(id, source.clone());
                }
                attempt
            }
            FileCacheMode::Shared(shared) => shared.source_with_files(id, &self.root, &self.files),
            FileCacheMode::Snapshot { snapshot, fallback } => {
                if snapshot.source_boundary() == self.files.source_boundary()
                    && let Some(source) = snapshot.get_source(id)
                {
                    return ReadAttempt::from_loaded(source);
                }
                fallback.source_with_files(id, &self.root, &self.files)
            }
            FileCacheMode::Candidate { files, parsed } => {
                if let Some(source) = files.source(id) {
                    return ReadAttempt::from_loaded(source);
                }
                parsed.source_from_observation(id, &self.root, files.read(id))
            }
        }
    }

    pub(crate) fn get_file(&self, id: FileId) -> ReadAttempt<Loaded<Bytes>> {
        match &self.cache {
            FileCacheMode::Local(local) => {
                if let Some(bytes) = local.files.read().get(&id) {
                    return ReadAttempt::from_loaded(bytes.clone());
                }
                let attempt = self.load_file(id);
                if let Ok(bytes) = &attempt.result {
                    local.files.write().insert(id, bytes.clone());
                }
                attempt
            }
            FileCacheMode::Shared(shared) => shared.file_with_files(id, &self.root, &self.files),
            FileCacheMode::Snapshot { snapshot, fallback } => {
                if snapshot.source_boundary() == self.files.source_boundary()
                    && let Some(bytes) = snapshot.get_file(id)
                {
                    return ReadAttempt::from_loaded(bytes);
                }
                fallback.file_with_files(id, &self.root, &self.files)
            }
            FileCacheMode::Candidate { files, parsed } => {
                if let Some(bytes) = files.file(id) {
                    return ReadAttempt::from_loaded(bytes);
                }
                parsed.file_from_observation(id, &self.root, files.read(id))
            }
        }
    }

    fn load_source(&self, id: FileId) -> ReadAttempt<Loaded<Source>> {
        self.files
            .read_attempt(id, &self.root)
            .map_loaded(|bytes| Ok(Source::new(id, decode_utf8(&bytes)?.to_owned())))
    }

    fn load_file(&self, id: FileId) -> ReadAttempt<Loaded<Bytes>> {
        self.files
            .read_attempt(id, &self.root)
            .map_loaded(|bytes| Ok(Bytes::new(bytes)))
    }
}

impl World for TypstWorld {
    fn library(&self) -> &LazyHash<Library> {
        match &self.library {
            LibraryMode::Global => &GLOBAL_LIBRARY,
            LibraryMode::Custom(lib) => lib,
        }
    }

    fn book(&self) -> &LazyHash<FontBook> {
        match &self.fonts {
            FontMode::None => empty_fontbook(),
            FontMode::Shared(fonts) => fonts.prepared().book(),
        }
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.get_source(id).result.map(|loaded| loaded.value)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.get_file(id).result.map(|loaded| loaded.value)
    }

    fn font(&self, index: usize) -> Option<Font> {
        match &self.fonts {
            FontMode::None => None,
            FontMode::Shared(fonts) => fonts.prepared().font(index),
        }
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.time.as_ref()?.today(offset)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use tempfile::TempDir;
    use typst::World;

    use super::{TypstWorld, WorldBuildError};
    use crate::world::SourceSnapshot;
    use crate::world::file::{CandidateFileSnapshot, FileResolver, SharedFileCache};

    #[test]
    fn resource_reads_keep_read_evidence() {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        let resource = directory.path().join("image.bin");
        fs::write(&main, "").unwrap();
        fs::write(&resource, b"original").unwrap();
        let mut world = TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        let id = crate::world::file::file_id("image.bin");
        let (bytes, observed) = world.read_file_with_evidence(id);
        assert_eq!(bytes.unwrap().as_slice(), b"original");
        let original = observed
            .reads
            .first()
            .expect("a successful read has evidence")
            .evidence()
            .clone();
        assert_eq!(original.digest(), crate::ContentDigest::of(b"original"));

        fs::write(&resource, b"replacement").unwrap();
        let (retained, retained_reads) = world.read_file_with_evidence(id);
        assert_eq!(retained.unwrap().as_slice(), b"original");
        assert_eq!(
            retained_reads.reads.first().map(|read| read.evidence()),
            Some(&original)
        );

        world.reset();
        let (current, current_reads) = world.read_file_with_evidence(id);
        assert_eq!(current.unwrap().as_slice(), b"replacement");
        assert_eq!(
            current_reads
                .reads
                .first()
                .expect("a successful read has evidence")
                .evidence()
                .digest(),
            crate::ContentDigest::of(b"replacement")
        );

        let (missing, attempted) =
            world.read_file_with_evidence(crate::world::file::file_id("missing.bin"));
        assert!(missing.is_err());
        assert_eq!(
            attempted.disk_reads[0].as_path(),
            world.root().join("missing.bin")
        );
    }

    #[test]
    fn snapshot_source_matches_file() {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        let original = b"\xef\xbb\xbf= Frozen";
        fs::write(&main, original).unwrap();
        let snapshot = SourceSnapshot::build(std::slice::from_ref(&main), directory.path())
            .unwrap()
            .into_snapshot_and_accessed()
            .0;
        let world = TypstWorld::builder(&main, directory.path())
            .with_snapshot(Arc::new(snapshot))
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        fs::write(&main, "= Changed").unwrap();

        assert_eq!(world.source(world.main()).unwrap().text(), "= Frozen");
        assert_eq!(world.file(world.main()).unwrap().as_slice(), original);
    }

    #[test]
    fn mismatched_snapshot_root_fails() {
        let world_directory = TempDir::new().unwrap();
        let snapshot_directory = TempDir::new().unwrap();
        let main = world_directory.path().join("main.typ");
        fs::write(&main, "= Main").unwrap();
        let snapshot = Arc::new(
            SourceSnapshot::build(&[], snapshot_directory.path())
                .unwrap()
                .into_snapshot_and_accessed()
                .0,
        );

        let snapshot_error = TypstWorld::builder(&main, world_directory.path())
            .with_snapshot(Arc::clone(&snapshot))
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .err()
            .expect("mismatched source snapshot unexpectedly built");
        assert!(matches!(
            snapshot_error,
            WorldBuildError::SnapshotRootMismatch { .. }
        ));

        let candidate = Arc::new(CandidateFileSnapshot::new(
            snapshot,
            Arc::new(FileResolver::new()),
        ));
        let candidate_error = TypstWorld::builder(&main, world_directory.path())
            .with_candidate_snapshot(candidate, Arc::new(SharedFileCache::new()))
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .err()
            .expect("mismatched candidate snapshot unexpectedly built");
        assert!(matches!(
            candidate_error,
            WorldBuildError::SnapshotRootMismatch { .. }
        ));
    }
}
