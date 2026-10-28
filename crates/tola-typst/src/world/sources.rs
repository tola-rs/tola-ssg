//! Explicit sources frozen for lock-free parallel compilation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::path::normalize_path;
use crate::session::AccessedDeps;
use crate::world::file::{FileResolver, Loaded, ReadAttempt, decode_utf8, file_id_from_path};

/// Failure to construct the explicit sources.
#[derive(Debug, thiserror::Error)]
pub enum SourceLoadError {
    /// A source could not be read or decoded.
    #[error("could not read a source")]
    File {
        /// Source path requested for the load.
        path: PathBuf,
        /// Structured file or decoding failure from the resolver.
        #[source]
        error: typst::diag::FileError,
    },
    /// An explicit source is outside the compilation root.
    #[error("a source this build reads is outside the compilation root")]
    OutsideRoot {
        /// Rejected source path.
        path: PathBuf,
    },
    /// Explicit sources cannot change their compilation root during refresh.
    #[error("the sources were captured for a different compilation root")]
    RootMismatch {
        /// Root of the existing sources.
        existing: PathBuf,
        /// Root requested for the refresh.
        requested: PathBuf,
    },
    /// The caller cancelled source loading.
    #[error("source reading stopped before the build finished")]
    Cancelled,
}

impl SourceLoadError {
    /// Source or root path associated with the failure, when one exists.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } | Self::OutsideRoot { path } => Some(path),
            Self::RootMismatch { requested, .. } => Some(requested),
            Self::Cancelled => None,
        }
    }

    /// Whether the caller cancelled this operation.
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// The explicit sources one load produced and the reads recorded while loading them.
pub struct SourceLoad {
    sources: ExplicitSources,
    accessed: AccessedDeps,
}

impl SourceLoad {
    /// Borrow the sources.
    pub fn sources(&self) -> &ExplicitSources {
        &self.sources
    }

    /// Borrow every read observed while loading the sources.
    pub fn accessed(&self) -> &AccessedDeps {
        &self.accessed
    }

    /// Consume the load into its sources and observed reads.
    pub fn into_sources_and_accessed(self) -> (ExplicitSources, AccessedDeps) {
        (self.sources, self.accessed)
    }
}

/// A failed source load and every read completed before failure.
#[derive(Debug)]
pub struct SourceLoadFailure {
    error: SourceLoadError,
    accessed: Box<AccessedDeps>,
}

impl SourceLoadFailure {
    /// Borrow every read observed before loading stopped.
    pub fn accessed(&self) -> &AccessedDeps {
        &self.accessed
    }

    /// Whether loading stopped because the caller cancelled it.
    pub fn is_cancelled(&self) -> bool {
        self.error.is_cancelled()
    }

    /// Consume the failure into its error and observed reads.
    pub fn into_error_and_accessed(self) -> (SourceLoadError, AccessedDeps) {
        (self.error, *self.accessed)
    }
}

impl std::fmt::Display for SourceLoadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for SourceLoadFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// The explicit sources one compilation reads, frozen for lock-free parallel access.
///
/// Built once before parallel compilation, then shared across all threads.
#[derive(Clone)]
pub struct ExplicitSources {
    base: Arc<FxHashMap<FileId, Loaded<FrozenSource>>>,
    overlay: Arc<FxHashMap<FileId, Loaded<FrozenSource>>>,
    members: Arc<FxHashSet<FileId>>,
    root: PathBuf,
    boundary: crate::world::SourceBoundary,
}

#[derive(Clone)]
struct FrozenSource {
    source: Source,
    bytes: Bytes,
}

/// Compact after this many overlay sources. Lookups access at most two hash maps.
const MAX_OVERLAY_SOURCES: usize = 64;
/// Limits temporary read results retained while loading explicit sources.
const LOAD_CHUNK_SIZE: usize = 128;

impl ExplicitSources {
    /// Load the explicitly listed source files.
    pub fn build(content_files: &[PathBuf], root: &Path) -> Result<SourceLoad, SourceLoadFailure> {
        Self::build_with_files(content_files, root, Arc::new(FileResolver::new()))
    }

    /// Load the same sources through a caller-supplied resolver.
    pub fn build_with_files(
        content_files: &[PathBuf],
        root: &Path,
        files: Arc<FileResolver>,
    ) -> Result<SourceLoad, SourceLoadFailure> {
        Self::build_with_files_and_cancellation(content_files, root, files, None)
    }

    /// Load the sources while checking cancellation around each source read.
    ///
    /// Reads completed by the parallel load remain available through
    /// [`SourceLoadFailure::accessed`] on cancellation.
    pub fn build_with_files_cancellable(
        content_files: &[PathBuf],
        root: &Path,
        files: Arc<FileResolver>,
        cancellation: &crate::BundleCancellation,
    ) -> Result<SourceLoad, SourceLoadFailure> {
        Self::build_with_files_and_cancellation(content_files, root, files, Some(cancellation))
    }

    fn build_with_files_and_cancellation(
        content_files: &[PathBuf],
        root: &Path,
        files: Arc<FileResolver>,
        cancellation: Option<&crate::BundleCancellation>,
    ) -> Result<SourceLoad, SourceLoadFailure> {
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            return Err(SourceLoadFailure::cancelled(AccessedDeps::default()));
        }
        validate_source_paths(content_files, files.source_boundary())
            .map_err(SourceLoadFailure::without_reads)?;
        let root = normalize_path(root);
        let content_files = normalize_source_paths(content_files, &root)
            .map_err(SourceLoadFailure::without_reads)?;

        let (sources, accessed) = load_sources(&content_files, &root, &files, cancellation, None)?;
        let members = sources.keys().copied().collect();

        Ok(SourceLoad {
            sources: Self {
                base: Arc::new(sources),
                overlay: Arc::new(FxHashMap::default()),
                members: Arc::new(members),
                root,
                boundary: files.source_boundary().clone(),
            },
            accessed,
        })
    }

    /// Freeze an exact source membership, loading only new members and `reload` paths.
    ///
    /// The immutable result shares unchanged parsed sources with `self`.
    /// Removed members cannot fall through to an older layer. Include every
    /// existing explicit source whose bytes may have changed in `reload`.
    pub fn refresh_with_files(
        &self,
        content_files: &[PathBuf],
        reload: &[PathBuf],
        root: &Path,
        files: Arc<FileResolver>,
    ) -> Result<SourceLoad, SourceLoadFailure> {
        self.refresh_with_files_and_cancellation(content_files, reload, root, files, None)
    }

    /// Refresh the sources while checking cancellation around each source read.
    ///
    /// The returned failure retains all reads completed before cancellation.
    pub fn refresh_with_files_cancellable(
        &self,
        content_files: &[PathBuf],
        reload: &[PathBuf],
        root: &Path,
        files: Arc<FileResolver>,
        cancellation: &crate::BundleCancellation,
    ) -> Result<SourceLoad, SourceLoadFailure> {
        self.refresh_with_files_and_cancellation(
            content_files,
            reload,
            root,
            files,
            Some(cancellation),
        )
    }

    fn refresh_with_files_and_cancellation(
        &self,
        content_files: &[PathBuf],
        reload: &[PathBuf],
        root: &Path,
        files: Arc<FileResolver>,
        cancellation: Option<&crate::BundleCancellation>,
    ) -> Result<SourceLoad, SourceLoadFailure> {
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            return Err(SourceLoadFailure::cancelled(AccessedDeps::default()));
        }
        let root = normalize_path(root);
        if root != self.root {
            return Err(SourceLoadFailure::without_reads(
                SourceLoadError::RootMismatch {
                    existing: self.root.clone(),
                    requested: root,
                },
            ));
        }
        if &self.boundary != files.source_boundary() {
            return Self::build_with_files_and_cancellation(
                content_files,
                &root,
                files,
                cancellation,
            );
        }
        validate_source_paths(content_files, files.source_boundary())
            .map_err(SourceLoadFailure::without_reads)?;

        let content_files = normalize_source_paths(content_files, &root)
            .map_err(SourceLoadFailure::without_reads)?;
        let desired = content_files
            .iter()
            .map(|path| {
                file_id_from_path(path, &root).expect("validated source path has a file ID")
            })
            .collect::<FxHashSet<_>>();
        let mut reload_paths = normalize_source_paths(reload, &root)
            .map_err(SourceLoadFailure::without_reads)?
            .into_iter()
            .filter(|path| file_id_from_path(path, &root).is_some_and(|id| desired.contains(&id)))
            .collect::<Vec<_>>();
        reload_paths.extend(content_files.iter().filter_map(|path| {
            let id = file_id_from_path(path, &root).expect("validated source path has a file ID");
            (!self.members.contains(&id)).then(|| path.clone())
        }));
        reload_paths.sort_unstable();
        reload_paths.dedup();

        let (refreshed, accessed) =
            load_sources(&reload_paths, &root, &files, cancellation, Some(self))?;
        let members = if desired == *self.members {
            Arc::clone(&self.members)
        } else {
            Arc::new(desired)
        };
        let removed = self.members.iter().any(|id| !members.contains(id));
        if refreshed.is_empty() && !removed {
            return Ok(SourceLoad {
                sources: Self {
                    base: Arc::clone(&self.base),
                    overlay: Arc::clone(&self.overlay),
                    members,
                    root,
                    boundary: self.boundary.clone(),
                },
                accessed,
            });
        }

        let mut overlay = (*self.overlay).clone();
        overlay.extend(refreshed);
        let (base, overlay) = if !removed && overlay.len() <= MAX_OVERLAY_SOURCES {
            (Arc::clone(&self.base), Arc::new(overlay))
        } else {
            let compacted = members
                .iter()
                .map(|id| {
                    (
                        *id,
                        overlay
                            .get(id)
                            .or_else(|| self.base.get(id))
                            .cloned()
                            .expect("every explicit source has a value"),
                    )
                })
                .collect();
            (Arc::new(compacted), Arc::new(FxHashMap::default()))
        };

        Ok(SourceLoad {
            sources: Self {
                base,
                overlay,
                members,
                root,
                boundary: self.boundary.clone(),
            },
            accessed,
        })
    }

    /// Gets a cached source by file ID.
    #[inline]
    pub(crate) fn get_source(&self, id: FileId) -> Option<Loaded<Source>> {
        self.get_frozen(id).map(|source| Loaded {
            value: source.value.source.clone(),
            read: source.read.clone(),
        })
    }

    /// Gets the exact source bytes frozen for the same file identity.
    pub(crate) fn get_file(&self, id: FileId) -> Option<Loaded<Bytes>> {
        self.get_frozen(id).map(|source| Loaded {
            value: source.value.bytes.clone(),
            read: source.read.clone(),
        })
    }

    fn get_frozen(&self, id: FileId) -> Option<&Loaded<FrozenSource>> {
        if !self.members.contains(&id) {
            return None;
        }
        self.overlay.get(&id).or_else(|| self.base.get(&id))
    }

    /// Return the normalized compilation root these sources belong to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn source_boundary(&self) -> &crate::world::SourceBoundary {
        &self.boundary
    }

    /// Whether these sources explicitly contain the given source identity.
    pub fn contains(&self, id: FileId) -> bool {
        self.members.contains(&id)
    }

    /// Number of explicit sources in the membership.
    #[inline]
    pub fn source_count(&self) -> usize {
        self.members.len()
    }
}

fn validate_source_paths(
    paths: &[PathBuf],
    boundary: &crate::world::SourceBoundary,
) -> Result<(), SourceLoadError> {
    for path in paths {
        boundary
            .check(path)
            .map_err(|error| SourceLoadError::File {
                path: path.clone(),
                error,
            })?;
    }
    Ok(())
}

fn normalize_source_paths(paths: &[PathBuf], root: &Path) -> Result<Vec<PathBuf>, SourceLoadError> {
    let paths = paths
        .iter()
        .map(|path| normalize_path(path))
        .collect::<Vec<_>>();
    if let Some(path) = paths
        .iter()
        .find(|path| file_id_from_path(path, root).is_none())
    {
        return Err(SourceLoadError::OutsideRoot { path: path.clone() });
    }
    Ok(paths)
}

fn load_sources(
    content_files: &[PathBuf],
    root: &Path,
    files: &FileResolver,
    cancellation: Option<&crate::BundleCancellation>,
    previous: Option<&ExplicitSources>,
) -> Result<(FxHashMap<FileId, Loaded<FrozenSource>>, AccessedDeps), SourceLoadFailure> {
    let mut sources = FxHashMap::default();
    let mut accessed = AccessedDeps::default();
    let mut first_error = None;
    let mut cancelled = false;

    for chunk in content_files.chunks(LOAD_CHUNK_SIZE) {
        #[cfg(feature = "parallel")]
        let loaded = {
            use rayon::prelude::*;
            chunk
                .par_iter()
                .map(|path| load_source(path, root, files, cancellation, previous))
                .collect::<Vec<_>>()
        };
        #[cfg(not(feature = "parallel"))]
        let loaded = chunk
            .iter()
            .map(|path| load_source(path, root, files, cancellation, previous));
        cancelled |= merge_loaded_sources(
            loaded,
            cancellation,
            &mut sources,
            &mut accessed,
            &mut first_error,
        );
        if cancelled {
            break;
        }
    }

    if cancelled {
        return Err(SourceLoadFailure::cancelled(accessed));
    }
    match first_error {
        Some(error) => Err(SourceLoadFailure {
            error,
            accessed: Box::new(accessed),
        }),
        None => Ok((sources, accessed)),
    }
}

fn merge_loaded_sources(
    loaded: impl IntoIterator<Item = (PathBuf, FileId, Option<ReadAttempt<Loaded<FrozenSource>>>)>,
    cancellation: Option<&crate::BundleCancellation>,
    sources: &mut FxHashMap<FileId, Loaded<FrozenSource>>,
    accessed: &mut AccessedDeps,
    first_error: &mut Option<SourceLoadError>,
) -> bool {
    let mut cancelled = false;
    for (path, id, attempt) in loaded {
        let Some(attempt) = attempt else {
            cancelled = true;
            continue;
        };
        accessed.reads.extend(attempt.reads);
        accessed.disk_reads.extend(attempt.disk_reads);
        accessed.package_checks.extend(attempt.package_checks);
        match attempt.result {
            Ok(source) => {
                sources.insert(id, source);
            }
            Err(error) if first_error.is_none() => {
                *first_error = Some(SourceLoadError::File { path, error });
            }
            Err(_) => {}
        }
        cancelled |= cancellation.is_some_and(|cancellation| cancellation.is_cancelled());
    }
    cancelled
}

fn load_source(
    path: &Path,
    root: &Path,
    files: &FileResolver,
    cancellation: Option<&crate::BundleCancellation>,
    previous: Option<&ExplicitSources>,
) -> (PathBuf, FileId, Option<ReadAttempt<Loaded<FrozenSource>>>) {
    let id = file_id_from_path(path, root).expect("validated source path has a file ID");
    if cancellation.is_some_and(|cancellation| cancellation.is_cancelled()) {
        return (path.to_path_buf(), id, None);
    }
    let previous = previous.and_then(|sources| sources.get_source(id).map(|loaded| loaded.value));
    let attempt = files.read_attempt(id, root).map_loaded(move |bytes| {
        let text = decode_utf8(&bytes)?;
        let source = match previous {
            Some(mut source) => {
                source.replace(text);
                source
            }
            None => Source::new(id, text.into()),
        };
        Ok(FrozenSource {
            source,
            bytes: Bytes::new(bytes),
        })
    });
    (path.to_path_buf(), id, Some(attempt))
}

impl SourceLoadFailure {
    fn without_reads(error: SourceLoadError) -> Self {
        Self {
            error,
            accessed: Box::default(),
        }
    }

    fn cancelled(accessed: AccessedDeps) -> Self {
        Self {
            error: SourceLoadError::Cancelled,
            accessed: Box::new(accessed),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use tempfile::TempDir;
    use typst::syntax::FileId;

    use super::{ExplicitSources, file_id_from_path};
    use crate::world::file::{FileProvider, FileResolver, FileTarget};

    struct CancelAfterResolution {
        cancellation: crate::BundleCancellation,
    }

    impl FileProvider for CancelAfterResolution {
        fn target(&self, _id: FileId) -> Option<FileTarget> {
            self.cancellation.cancel();
            Some(FileTarget::Bytes(Arc::from(&b"source"[..])))
        }
    }

    fn explicit_sources(paths: &[PathBuf], root: &Path) -> ExplicitSources {
        ExplicitSources::build(paths, root)
            .unwrap()
            .into_sources_and_accessed()
            .0
    }

    #[test]
    fn explicit_sources_freeze_exactly_listed_files() {
        let dir = TempDir::new().unwrap();
        let main = dir.path().join("main.typ");
        fs::write(&main, "#import \"shared.typ\": value\n#value").unwrap();
        let shared = dir.path().join("shared.typ");
        fs::write(&shared, "#let value = 1").unwrap();
        let root = dir.path().join("root.typ");
        fs::write(&root, "#let root = 2").unwrap();

        let frozen = explicit_sources(std::slice::from_ref(&main), dir.path());

        assert_eq!(frozen.source_count(), 1);
        assert_eq!(frozen.root(), super::normalize_path(dir.path()));
        assert!(frozen.contains(file_id_from_path(&main, dir.path()).unwrap()));
        assert!(!frozen.contains(file_id_from_path(&shared, dir.path()).unwrap()));

        let frozen = explicit_sources(&[main, shared, root], dir.path());

        assert_eq!(frozen.source_count(), 3);
    }

    #[test]
    fn cancelled_load_keeps_earlier_reads() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.typ");
        let cancellation = crate::BundleCancellation::default();
        let files = FileResolver::new().with_provider(CancelAfterResolution {
            cancellation: cancellation.clone(),
        });

        let failure = ExplicitSources::build_with_files_cancellable(
            &[source],
            dir.path(),
            Arc::new(files),
            &cancellation,
        )
        .err()
        .expect("load cancelled after resolution unexpectedly loaded");

        assert!(failure.is_cancelled());
        assert_eq!(failure.accessed().reads.len(), 1);
        assert!(failure.accessed().disk_reads.is_empty());
    }

    #[test]
    fn refresh_reads_only_changed_sources() {
        let dir = TempDir::new().unwrap();
        let first = dir.path().join("first.typ");
        let second = dir.path().join("second.typ");
        let third = dir.path().join("third.typ");
        fs::write(&first, "first-v1").unwrap();
        fs::write(&second, "second-v1").unwrap();

        let initial = explicit_sources(&[first.clone(), second.clone()], dir.path());
        fs::write(&first, "first-v2").unwrap();
        fs::write(&third, "third-v1").unwrap();
        let loaded = initial
            .refresh_with_files(
                &[first.clone(), second.clone(), third.clone()],
                std::slice::from_ref(&first),
                dir.path(),
                Arc::new(FileResolver::new()),
            )
            .unwrap();
        assert_eq!(loaded.accessed().disk_reads.len(), 2);
        let refreshed = loaded.into_sources_and_accessed().0;

        assert_eq!(refreshed.source_count(), 3);
        let source_text = |sources: &ExplicitSources, path: &std::path::Path| {
            let id = file_id_from_path(path, dir.path()).unwrap();
            sources.get_source(id).unwrap().value.text().to_owned()
        };
        assert_eq!(source_text(&refreshed, &first), "first-v2");
        assert_eq!(source_text(&refreshed, &second), "second-v1");
        assert_eq!(source_text(&refreshed, &third), "third-v1");
        assert_eq!(source_text(&initial, &first), "first-v1");
    }
}
