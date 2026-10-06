//! Builder pattern for `TypstWorld`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;
use typst::Library;
use typst::foundations::Dict;
use typst::utils::LazyHash;

use super::core::TypstWorld;
use super::snapshot::SourceSnapshot;
use super::strategy::{FileCacheMode, FontMode, LibraryMode};
use crate::world::file::{CandidateFileSnapshot, FileResolver, SharedFileCache};
use crate::world::font::{FontLoadError, FontStore};

/// Invalid configuration for a Typst world.
#[derive(Debug, Error)]
pub enum WorldBuildError {
    /// The main source cannot be represented relative to the compilation root.
    #[error("the main source is outside the site root")]
    MainOutsideRoot {
        /// Normalized main source path.
        main: PathBuf,
        /// Normalized compilation root.
        root: PathBuf,
    },

    /// No file cache strategy was selected.
    #[error("no file cache configured for the Typst world")]
    MissingFileCache,

    /// No font strategy was selected.
    #[error("no fonts configured; call `with_fonts(...)` or `no_fonts()`")]
    MissingFonts,

    /// A frozen file view belongs to another compilation root.
    #[error("the sources belong to a different compilation root")]
    SnapshotRootMismatch {
        /// Root of the frozen file view.
        snapshot: PathBuf,
        /// Normalized root requested for the world.
        world: PathBuf,
    },

    /// A physical source violates the caller's source boundary.
    #[error("{0}")]
    Source(#[source] typst::diag::FileError),

    /// A candidate snapshot was captured under different source restrictions.
    #[error("the sources belong to a different source boundary")]
    SnapshotBoundaryMismatch,

    /// A configured font resource could not be prepared.
    #[error("{0}")]
    Font(#[from] FontLoadError),

    /// The caller cancelled input preparation.
    #[error("operation cancelled")]
    Cancelled,

    /// A fixed Typst datetime contained only a time of day.
    #[error("the build date has no calendar day")]
    FixedTimeWithoutDate,
}

/// Builder for configuring `TypstWorld`.
///
/// Use `TypstWorld::builder()` to create a builder.
pub struct WorldBuilder {
    main_path: PathBuf,
    root: PathBuf,
    files: Arc<FileResolver>,
    cache: Option<FileCacheMode>,
    fonts: Option<FontMode>,
    library: LibraryMode,
    time: TimeMode,
}

enum TimeMode {
    None,
    Fixed(typst::foundations::Datetime),
    System,
}

impl WorldBuilder {
    pub(crate) fn new(main_path: &Path, root: &Path) -> Self {
        Self {
            main_path: main_path.to_path_buf(),
            root: root.to_path_buf(),
            files: Arc::new(FileResolver::new()),
            cache: None,
            fonts: None,
            library: LibraryMode::Global,
            time: TimeMode::None,
        }
    }

    /// Use an explicit file resolver for this world.
    pub fn with_files(mut self, files: Arc<FileResolver>) -> Self {
        self.files = files;
        self
    }

    /// Use task-local cache (no sharing between compilations).
    pub fn with_local_cache(mut self) -> Self {
        self.cache = Some(FileCacheMode::local());
        self
    }

    /// Use shared cache with lock-based synchronization.
    ///
    /// Suits hot reload and incremental updates, where files change frequently.
    pub fn with_shared_cache(mut self, cache: Arc<SharedFileCache>) -> Self {
        self.cache = Some(FileCacheMode::shared(cache));
        self
    }

    /// Use pre-built immutable snapshot for lock-free parallel access.
    ///
    /// Suits batch compilation of pre-scanned sources.
    pub fn with_snapshot(mut self, snapshot: Arc<SourceSnapshot>) -> Self {
        self.cache = Some(FileCacheMode::snapshot(snapshot));
        self
    }

    /// Use one candidate-scoped view of explicit and first-observed files.
    pub fn with_candidate_snapshot(
        mut self,
        files: Arc<CandidateFileSnapshot>,
        parsed: Arc<SharedFileCache>,
    ) -> Self {
        self.cache = Some(FileCacheMode::candidate(files, parsed));
        self
    }

    /// Use an already resolved file access policy.
    /// Disable font loading.
    ///
    /// Suits scans and queries, which do not lay out content.
    pub fn no_fonts(mut self) -> Self {
        self.fonts = Some(FontMode::None);
        self
    }

    /// Use shared fonts.
    pub fn with_fonts(mut self, fonts: Arc<FontStore>) -> Self {
        self.fonts = Some(FontMode::Shared(fonts));
        self
    }

    /// Configure `sys.inputs` for the compilation.
    pub fn with_inputs<I, K, V>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<typst::foundations::Str>,
        V: typst::foundations::IntoValue,
    {
        let dict: Dict = inputs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into_value()))
            .collect();
        self.library = LibraryMode::custom(dict);
        self
    }

    /// Configure `sys.inputs` from a pre-built `Dict`.
    pub fn with_inputs_dict(mut self, inputs: Dict) -> Self {
        self.library = LibraryMode::custom(inputs);
        self
    }

    /// Use one immutable Typst library across multiple worlds.
    ///
    /// Worlds share the exact library identity but keep separate file and diagnostic sessions.
    pub fn with_shared_library(mut self, library: Arc<LazyHash<Library>>) -> Self {
        self.library = LibraryMode::Custom(library);
        self
    }

    /// Set a fixed date and time for `datetime.today()`.
    ///
    /// If not set, `datetime.today()` returns `None` (compile error).
    /// This keeps builds reproducible by default.
    pub fn with_fixed_datetime(mut self, datetime: typst::foundations::Datetime) -> Self {
        self.time = TimeMode::Fixed(datetime);
        self
    }

    /// Read system time for `datetime.today()`.
    pub fn with_system_time(mut self) -> Self {
        self.time = TimeMode::System;
        self
    }

    /// Prepare this world using the caller's cancellation for input loading.
    pub fn build(
        self,
        cancellation: &crate::BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        let cache = self.cache.ok_or(WorldBuildError::MissingFileCache)?;
        let fonts = self.fonts.ok_or(WorldBuildError::MissingFonts)?;
        if cancellation.is_cancelled() {
            return Err(WorldBuildError::Cancelled);
        }
        if let FontMode::Shared(store) = &fonts {
            store.load(cancellation)?;
        }
        TypstWorld::new(
            &self.main_path,
            &self.root,
            self.files,
            cache,
            fonts,
            self.library,
            match self.time {
                TimeMode::None => None,
                TimeMode::Fixed(datetime) => Some(
                    typst_kit::datetime::Time::fixed(datetime)
                        .map_err(|_| WorldBuildError::FixedTimeWithoutDate)?,
                ),
                TimeMode::System => Some(typst_kit::datetime::Time::system()),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::{WorldBuildError, WorldBuilder};
    use crate::{BundleCancellation, FontOptions, FontStore};

    /// A compilation root whose `main.typ` holds `source`, kept alive by the returned `TempDir`.
    fn source_directory(source: &str) -> (TempDir, PathBuf) {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        fs::write(&main, source).unwrap();
        (directory, main)
    }

    fn build_error(
        root: &Path,
        main: &Path,
        configure: impl FnOnce(WorldBuilder) -> WorldBuilder,
    ) -> WorldBuildError {
        match configure(WorldBuilder::new(main, root)).build(&BundleCancellation::default()) {
            Ok(_) => panic!("world construction unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    #[test]
    fn build_requires_file_cache() {
        let (directory, main) = source_directory("Hello");

        assert!(matches!(
            build_error(directory.path(), &main, |builder| builder.no_fonts()),
            WorldBuildError::MissingFileCache
        ));
    }

    #[test]
    fn build_requires_font_strategy() {
        let (directory, main) = source_directory("Hello");

        assert!(matches!(
            build_error(directory.path(), &main, WorldBuilder::with_local_cache),
            WorldBuildError::MissingFonts
        ));
    }

    #[test]
    fn main_source_outside_root_fails() {
        let (_outside, main) = source_directory("Hello");
        let root = TempDir::new().unwrap();

        assert!(matches!(
            build_error(root.path(), &main, |builder| {
                builder.with_local_cache().no_fonts()
            }),
            WorldBuildError::MainOutsideRoot { .. }
        ));
    }

    #[test]
    fn fixed_time_without_date_fails() {
        let (directory, main) = source_directory("Hello");

        assert!(matches!(
            build_error(directory.path(), &main, |builder| {
                builder.with_local_cache().no_fonts().with_fixed_datetime(
                    typst::foundations::Datetime::from_hms(12, 34, 56).unwrap(),
                )
            }),
            WorldBuildError::FixedTimeWithoutDate
        ));
    }

    #[test]
    fn fixed_datetime_uses_offset_rules() {
        use typst::World;
        use typst::foundations::{Datetime, Duration};

        let (directory, main) = source_directory("Hello");
        let world = WorldBuilder::new(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .with_fixed_datetime(Datetime::from_ymd_hms(2026, 8, 17, 23, 30, 0).unwrap())
            .build(&BundleCancellation::default())
            .expect("valid fixed datetime world");

        assert_eq!(
            world.today(Some(Duration::construct(0, 0, 1, 0, 0))),
            Datetime::from_ymd(2026, 8, 18)
        );
    }

    #[test]
    fn reset_refreshes_cached_values() {
        use typst::World;

        let (directory, main) = source_directory("= First");
        let mut world = WorldBuilder::new(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&BundleCancellation::default())
            .expect("valid world");
        assert_eq!(world.source(world.main()).unwrap().text(), "= First");
        let first =
            String::from_utf8(crate::compile_world(&world).unwrap().html().unwrap()).unwrap();
        assert!(first.contains("First"), "{first}");

        fs::write(&main, "= Second").unwrap();
        assert_eq!(world.source(world.main()).unwrap().text(), "= First");

        world.reset();

        assert_eq!(world.source(world.main()).unwrap().text(), "= Second");
        let second =
            String::from_utf8(crate::compile_world(&world).unwrap().html().unwrap()).unwrap();
        assert!(second.contains("Second"), "{second}");
        assert!(!second.contains("First"), "{second}");
    }

    #[test]
    fn cancelled_preparation_leaves_fonts_unloaded() {
        let (directory, main) = source_directory("Hello");
        let fonts = Arc::new(FontStore::with_options(
            FontOptions::new()
                .with_system_fonts(false)
                .with_embedded_fonts(false),
        ));
        let cancelled = BundleCancellation::new();
        cancelled.cancel();
        let error = WorldBuilder::new(&main, directory.path())
            .with_local_cache()
            .with_fonts(Arc::clone(&fonts))
            .build(&cancelled)
            .err()
            .expect("preparation was cancelled");
        assert!(crate::CompileError::from(error).is_cancelled());
        assert!(!fonts.is_loaded());

        WorldBuilder::new(&main, directory.path())
            .with_local_cache()
            .with_fonts(Arc::clone(&fonts))
            .build(&BundleCancellation::new())
            .unwrap();
        assert!(fonts.is_loaded());
    }

    #[test]
    fn fontless_preparation_observes_cancellation() {
        let (directory, main) = source_directory("Hello");
        let cancelled = BundleCancellation::new();
        cancelled.cancel();
        let error = WorldBuilder::new(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&cancelled)
            .err()
            .expect("preparation was cancelled");
        assert!(matches!(error, WorldBuildError::Cancelled));
    }

    #[test]
    fn required_strategies_precede_cancellation() {
        let (directory, main) = source_directory("Hello");
        let cancelled = BundleCancellation::new();
        cancelled.cancel();
        assert!(matches!(
            WorldBuilder::new(&main, directory.path())
                .no_fonts()
                .build(&cancelled)
                .err(),
            Some(WorldBuildError::MissingFileCache),
        ));
        assert!(matches!(
            WorldBuilder::new(&main, directory.path())
                .with_local_cache()
                .build(&cancelled)
                .err(),
            Some(WorldBuildError::MissingFonts),
        ));
    }
}
