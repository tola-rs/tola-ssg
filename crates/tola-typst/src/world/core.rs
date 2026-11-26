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
//! // Build: with fonts, shared cache
//! let world = TypstWorld::builder(path, root)
//!     .with_sources(sources)
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

use super::builder::{MainSource, WorldBuildError, WorldBuilder};
use super::path::normalize_path;
use super::strategy::{FileAccess, FileCacheMode, FontMode, LibraryMode};
use crate::world::file::{
    FileStore, FileView, Loaded, ReadAttempt, SharedFileCache, file_id_from_path,
};
use crate::world::library::GLOBAL_LIBRARY;

static EMPTY_FONTBOOK: OnceLock<LazyHash<FontBook>> = OnceLock::new();

fn empty_fontbook() -> &'static LazyHash<FontBook> {
    EMPTY_FONTBOOK.get_or_init(|| LazyHash::new(FontBook::new()))
}

/// Typst world configured through [`Self::builder`].
pub struct TypstWorld {
    root: PathBuf,
    main: FileId,
    files: FileAccess,
    parsed: Arc<SharedFileCache>,
    fonts: FontMode,
    library: LibraryMode,
    time: Option<typst_kit::datetime::Time>,
    cancellation: crate::BundleCancellation,
}

impl TypstWorld {
    /// Select the file cache and font policies this world compiles with.
    pub fn builder(main_path: &Path, root: &Path) -> WorldBuilder {
        WorldBuilder::new(main_path, root)
    }

    /// Compile a complete file identity while retaining the project's root.
    ///
    /// This allows a declared package's site driver or model to be the main
    /// source without copying it into the project or changing package-relative
    /// imports. The resolver validates the selected source when it is read.
    pub fn builder_with_id(main: FileId, project_root: &Path) -> WorldBuilder {
        WorldBuilder::new_id(main, project_root)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        main: MainSource,
        root: &Path,
        files: Arc<FileStore>,
        cache: FileCacheMode,
        fonts: FontMode,
        library: LibraryMode,
        time: Option<typst_kit::datetime::Time>,
        cancellation: crate::BundleCancellation,
    ) -> Result<Self, WorldBuildError> {
        let root = normalize_path(root);
        let main = match main {
            MainSource::Path(main_path) => {
                files
                    .source_boundary()
                    .check(&main_path)
                    .map_err(WorldBuildError::Source)?;
                let main_abs = normalize_path(&main_path);
                file_id_from_path(&main_abs, &root).ok_or_else(|| {
                    WorldBuildError::MainOutsideRoot {
                        main: main_abs,
                        root: root.clone(),
                    }
                })?
            }
            MainSource::Id(main) => main,
        };
        let (files, parsed) = match cache {
            FileCacheMode::Local => (
                FileAccess::Local(Arc::new(FileView::from_resolver(&root, files))),
                Arc::new(SharedFileCache::new()),
            ),
            FileCacheMode::Shared(parsed) => (
                FileAccess::Live {
                    files,
                    sources: None,
                },
                parsed,
            ),
            FileCacheMode::Sources { sources, fallback } => {
                if sources.root() != root {
                    return Err(WorldBuildError::RootMismatch {
                        frozen: sources.root().to_path_buf(),
                        world: root,
                    });
                }
                (
                    FileAccess::Live {
                        files,
                        sources: Some(sources),
                    },
                    fallback,
                )
            }
            FileCacheMode::Frozen { files, parsed } => {
                if files.root() != root {
                    return Err(WorldBuildError::RootMismatch {
                        frozen: files.root().to_path_buf(),
                        world: root,
                    });
                }
                (FileAccess::Frozen(files), parsed)
            }
        };

        Ok(Self {
            root,
            main,
            files,
            parsed,
            fonts,
            library,
            time,
            cancellation,
        })
    }

    pub(crate) fn cancellation(&self) -> &crate::BundleCancellation {
        &self.cancellation
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

    /// Local package bindings fixed when this world's file view was created.
    pub fn local_packages(&self) -> &Arc<crate::world::file::LocalPackages> {
        self.resolver().local_packages()
    }

    /// Resolve a declared local source for navigation, including an unsaved overlay.
    /// This resolves identity only; it does not read newer bytes into the compiled world.
    pub fn local_package_path(&self, id: FileId) -> FileResult<Option<PathBuf>> {
        self.resolver().local_package_path(id)
    }

    fn resolver(&self) -> &FileStore {
        match &self.files {
            FileAccess::Local(files) | FileAccess::Frozen(files) => files.resolver(),
            FileAccess::Live { files, .. } => files,
        }
    }

    /// Read bytes through this world's file view and retain their dependency evidence.
    ///
    /// Consumers extending a compiled result with derived resources must use the
    /// same file view and hold these inputs to their publication checks. Failures
    /// retain attempted paths and package-selection evidence in the second value.
    pub fn read_file_with_evidence(
        &self,
        fid: FileId,
    ) -> (FileResult<Bytes>, crate::session::AccessedDeps) {
        let attempt = self.get_file(fid);
        let res = attempt
            .result
            .as_ref()
            .map(|loaded| loaded.value.clone())
            .map_err(Clone::clone);
        (res, attempt.into_deps())
    }

    /// Reset per-world inputs before another compilation.
    ///
    /// Local file observations are discarded. Shared worlds revalidate on access;
    /// externally supplied file views keep their observations. Resets system time but
    /// leaves fixed datetimes unchanged.
    pub fn reset(&mut self) {
        if let FileAccess::Local(files) = &mut self.files {
            *files = Arc::new(FileView::from_resolver(
                &self.root,
                Arc::clone(files.resolver()),
            ));
            self.parsed = Arc::new(SharedFileCache::new());
        }
        if let Some(time) = &mut self.time {
            time.reset();
        }
    }

    pub(crate) fn file_view(&self) -> Arc<FileView> {
        match &self.files {
            FileAccess::Local(files) | FileAccess::Frozen(files) => Arc::clone(files),
            FileAccess::Live { files, sources } => Arc::new(match sources {
                Some(sources) => FileView::new(Arc::clone(sources), Arc::clone(files)),
                None => FileView::from_resolver(&self.root, Arc::clone(files)),
            }),
        }
    }

    pub(crate) fn source_from_read(
        &self,
        files: &FileView,
        fid: FileId,
        attempt: ReadAttempt<Bytes>,
    ) -> ReadAttempt<Loaded<Source>> {
        if let Some(src) = files.source(fid) {
            return attempt.map_loaded(|_| Ok(src.value));
        }
        self.parsed.load_source(fid, &self.root, attempt)
    }

    pub(crate) fn file_from_read(
        &self,
        files: &FileView,
        fid: FileId,
        attempt: ReadAttempt<Bytes>,
    ) -> ReadAttempt<Loaded<Bytes>> {
        if let Some(buf) = files.file(fid) {
            return attempt.map_loaded(|_| Ok(buf.value));
        }
        self.parsed.load_file(fid, &self.root, attempt)
    }

    pub(crate) fn get_source(&self, fid: FileId) -> ReadAttempt<Loaded<Source>> {
        let files = self.file_view();
        self.source_from_read(&files, fid, files.read_cancellable(fid, &self.cancellation))
    }

    pub(crate) fn get_file(&self, fid: FileId) -> ReadAttempt<Loaded<Bytes>> {
        let files = self.file_view();
        self.file_from_read(&files, fid, files.read_cancellable(fid, &self.cancellation))
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

    fn source(&self, fid: FileId) -> FileResult<Source> {
        self.get_source(fid).result.map(|loaded| loaded.value)
    }

    fn file(&self, fid: FileId) -> FileResult<Bytes> {
        self.get_file(fid).result.map(|loaded| loaded.value)
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

    use super::{TypstWorld, WorldBuildError};
    use crate::world::ExplicitSources;
    use crate::world::file::{FileStore, FileView, SharedFileCache};

    #[test]
    fn resource_read_evidence() {
        let dir = TempDir::new().unwrap();
        let main = dir.path().join("main.typ");
        let res = dir.path().join("image.bin");
        fs::write(&main, "").unwrap();
        fs::write(&res, b"original").unwrap();
        let mut world = TypstWorld::builder(&main, dir.path())
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        let parsed = Arc::downgrade(&world.parsed);
        let fid = crate::world::file::file_id("image.bin");
        let (buf, obs) = world.read_file_with_evidence(fid);
        assert_eq!(buf.unwrap().as_slice(), b"original");
        let original = obs
            .reads
            .first()
            .expect("a successful read has evidence")
            .evidence()
            .clone();
        assert_eq!(original.digest(), crate::ContentDigest::of(b"original"));

        fs::write(&res, b"replacement").unwrap();
        let (retained, retained_reads) = world.read_file_with_evidence(fid);
        assert_eq!(retained.unwrap().as_slice(), b"original");
        assert_eq!(
            retained_reads.reads.first().map(|read| read.evidence()),
            Some(&original)
        );

        world.reset();
        assert!(parsed.upgrade().is_none());
        let (current, current_reads) = world.read_file_with_evidence(fid);
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
    fn mismatched_root() {
        let world_dir = TempDir::new().unwrap();
        let src_dir = TempDir::new().unwrap();
        let main = world_dir.path().join("main.typ");
        fs::write(&main, "= Main").unwrap();
        let sources = Arc::new(
            ExplicitSources::build(&[], src_dir.path())
                .unwrap()
                .into_sources_and_accessed()
                .0,
        );

        let src_err = TypstWorld::builder(&main, world_dir.path())
            .with_sources(Arc::clone(&sources))
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .err()
            .expect("mismatched sources unexpectedly built");
        assert!(matches!(src_err, WorldBuildError::RootMismatch { .. }));

        let files = Arc::new(FileView::new(sources, Arc::new(FileStore::new())));
        let view_err = TypstWorld::builder(&main, world_dir.path())
            .with_file_view(files, Arc::new(SharedFileCache::new()))
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .err()
            .expect("mismatched file view unexpectedly built");
        assert!(matches!(view_err, WorldBuildError::RootMismatch { .. }));
    }
}
