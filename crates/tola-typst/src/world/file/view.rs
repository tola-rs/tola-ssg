//! Explicit and first-observed compiler files.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use rustc_hash::FxHashMap;
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::{FileRead, FileStore, Loaded, ReadAttempt};
use crate::BundleCancellation;
use crate::world::ExplicitSources;
use crate::world::package::{PackageCheck, PackageSpec, PackageStore};

type ObservationCell<V> = Arc<Mutex<Option<V>>>;
type FileObservation = ObservationCell<ReadAttempt<Bytes>>;
type PackageObservationCell = ObservationCell<PackageObservation>;

/// Return the cell holding the first observation of `key`.
///
/// The shared cell is inserted under the write lock, so concurrent observers
/// agree on which observation is retained for `key`.
fn observation_cell<K, V>(
    cells: &RwLock<FxHashMap<K, ObservationCell<V>>>,
    key: &K,
) -> ObservationCell<V>
where
    K: Eq + std::hash::Hash + Clone,
{
    if let Some(cell) = cells.read().get(key) {
        return Arc::clone(cell);
    }
    let mut cells = cells.write();
    Arc::clone(cells.entry(key.clone()).or_default())
}

fn observe<V: Clone>(
    cell: &Mutex<Option<V>>,
    cancellation: &BundleCancellation,
    read: impl FnOnce() -> V,
) -> Option<V> {
    let mut stored = loop {
        if cancellation.is_cancelled() {
            return None;
        }
        if let Some(stored) = cell.try_lock_for(Duration::from_millis(25)) {
            break stored;
        }
    };
    if let Some(value) = stored.as_ref() {
        return Some(value.clone());
    }
    let value = read();
    if !cancellation.is_cancelled() {
        *stored = Some(value.clone());
    }
    Some(value)
}

/// One file view shared by compilations that must observe the same inputs.
///
/// Explicit source bytes come from the supplied [`ExplicitSources`]. Every
/// other successful read, failure, and package selection is retained from its
/// first observation. Create another view to observe changed files.
/// Different files can be first observed at different times.
pub struct FileView {
    root: PathBuf,
    sources: Option<Arc<ExplicitSources>>,
    files: Arc<FileStore>,
    observations: RwLock<FxHashMap<FileId, FileObservation>>,
    packages: PackageObservations,
}

impl FileView {
    /// Retain explicit sources and the resolver's first observations.
    ///
    /// Sources captured under different restrictions are read again through the resolver.
    pub fn new(sources: Arc<ExplicitSources>, files: Arc<FileStore>) -> Self {
        let same_boundary = sources.source_boundary() == files.source_boundary();
        let mut view = Self::from_resolver(sources.root(), files);
        view.sources = same_boundary.then_some(sources);
        view
    }

    /// Normalized compilation root owned by this file view.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn from_resolver(root: &Path, files: Arc<FileStore>) -> Self {
        Self {
            root: root.to_path_buf(),
            sources: None,
            files,
            observations: RwLock::new(FxHashMap::default()),
            packages: PackageObservations::default(),
        }
    }

    pub(crate) fn resolver(&self) -> &Arc<FileStore> {
        &self.files
    }

    pub(crate) fn source(&self, fid: FileId) -> Option<Loaded<Source>> {
        self.sources.as_ref()?.get_source(fid)
    }

    pub(crate) fn file(&self, fid: FileId) -> Option<Loaded<Bytes>> {
        self.sources.as_ref()?.get_file(fid)
    }

    #[cfg(test)]
    pub(crate) fn read(&self, fid: FileId) -> ReadAttempt<Bytes> {
        self.read_cancellable(fid, &BundleCancellation::default())
    }

    pub(crate) fn read_cancellable(
        &self,
        fid: FileId,
        cancel: &BundleCancellation,
    ) -> ReadAttempt<Bytes> {
        if cancel.is_cancelled() {
            return ReadAttempt::bare(Err(super::cancelled()));
        }
        if let Some(buf) = self.file(fid) {
            return ReadAttempt {
                result: Ok(buf.value),
                read: Some(buf.read),
                disk_reads: Vec::new(),
                package_checks: Vec::new(),
            };
        }
        observe(&observation_cell(&self.observations, &fid), cancel, || {
            let attempt =
                self.files
                    .read_attempt_with_packages(fid, self.root(), (&self.packages, cancel));
            ReadAttempt {
                result: attempt.result.map(Bytes::new),
                read: attempt.read,
                disk_reads: attempt.disk_reads,
                package_checks: attempt.package_checks,
            }
        })
        .unwrap_or_else(|| ReadAttempt::bare(Err(super::cancelled())))
    }

    /// Observe a file and compare its exact successful read identity.
    pub fn matches_read(&self, fid: FileId, expected: &FileRead) -> bool {
        self.matching_accessed(fid, expected, &BundleCancellation::default())
            .is_some()
    }

    /// Observe a file and return its inputs when the successful identity matches.
    pub fn matching_accessed(
        &self,
        fid: FileId,
        expected: &FileRead,
        cancel: &BundleCancellation,
    ) -> Option<crate::session::AccessedDeps> {
        let obs = self.read_cancellable(fid, cancel);
        (obs.result.is_ok() && obs.read.as_ref() == Some(expected)).then(|| obs.into_deps())
    }
}

#[derive(Default)]
pub(crate) struct PackageObservations {
    observations: RwLock<FxHashMap<PackageSpec, PackageObservationCell>>,
}

#[derive(Clone)]
pub(crate) struct PackageObservation {
    pub(crate) directory: typst::diag::FileResult<PathBuf>,
    pub(crate) checks: Vec<PackageCheck>,
}

impl PackageObservations {
    pub(crate) fn observe(
        &self,
        pkg: &PackageSpec,
        store: &PackageStore,
        cancel: &BundleCancellation,
    ) -> PackageObservation {
        observe(
            &observation_cell(&self.observations, pkg),
            cancel,
            || match store.prepare_with_cancellation(pkg, cancel) {
                Ok(prepared) => {
                    let (dir, checks) = prepared.into_directory_and_checks();
                    PackageObservation {
                        directory: Ok(dir),
                        checks,
                    }
                }
                Err(failure) => {
                    let (err, checks) = failure.into_error_and_checks();
                    PackageObservation {
                        directory: Err(err),
                        checks,
                    }
                }
            },
        )
        .unwrap_or_else(|| PackageObservation {
            directory: Err(super::cancelled()),
            checks: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

    use super::*;

    #[test]
    fn observe_cancelled_leaves_empty() {
        let cell = Mutex::default();
        let cancel = BundleCancellation::new();
        assert_eq!(
            observe(&cell, &cancel, || {
                cancel.cancel();
                1
            }),
            Some(1)
        );
        assert_eq!(observe(&cell, &BundleCancellation::new(), || 2), Some(2));
    }

    fn fid(path: &str) -> FileId {
        FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(path).unwrap(),
        ))
    }

    fn file_view(root: &Path, files: Arc<FileStore>) -> FileView {
        let sources = ExplicitSources::build(&[], root)
            .unwrap()
            .into_sources_and_accessed()
            .0;
        FileView::new(Arc::new(sources), files)
    }

    #[test]
    fn file_view_pins_first_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("value.txt");
        let files = Arc::new(FileStore::new());

        fs::write(&path, "first").unwrap();
        let present = file_view(dir.path(), Arc::clone(&files));
        assert_eq!(
            present.read(fid("value.txt")).result.unwrap().as_ref(),
            b"first"
        );
        fs::write(&path, "second").unwrap();
        assert_eq!(
            present.read(fid("value.txt")).result.unwrap().as_ref(),
            b"first"
        );
        assert_eq!(
            file_view(dir.path(), Arc::clone(&files))
                .read(fid("value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"second"
        );

        fs::remove_file(&path).unwrap();
        let absent = file_view(dir.path(), Arc::clone(&files));
        assert!(absent.read(fid("value.txt")).result.is_err());
        fs::write(&path, "created").unwrap();
        assert!(absent.read(fid("value.txt")).result.is_err());
        assert_eq!(
            file_view(dir.path(), files)
                .read(fid("value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"created"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_view_pins_pkg_symlink() {
        use typst::syntax::package::PackageSpec;

        fn pkg_id(pkg: &PackageSpec, path: &str) -> FileId {
            FileId::new(RootedPath::new(
                VirtualRoot::Package(pkg.clone()),
                VirtualPath::new(path).unwrap(),
            ))
        }

        let dir = TempDir::new().unwrap();
        let pkgs = dir.path().join("packages");
        let pkg_link = pkgs.join("local/demo/1.0.0");
        let first_dir = dir.path().join("first");
        let second_dir = dir.path().join("second");
        fs::create_dir_all(pkg_link.parent().unwrap()).unwrap();
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        fs::write(first_dir.join("value.txt"), "first value").unwrap();
        fs::write(second_dir.join("value.txt"), "second value").unwrap();
        std::os::unix::fs::symlink(&first_dir, &pkg_link).unwrap();
        let locations =
            crate::world::package::PackageLocations::from_absolute_roots(Some(pkgs), None).unwrap();
        let files = Arc::new(FileStore::from_package_locations(
            locations,
            crate::world::package::PackageFetchMode::LocalOnly,
        ));
        let pkg: PackageSpec = "@local/demo:1.0.0".parse().unwrap();
        let frozen = file_view(dir.path(), Arc::clone(&files));
        assert_eq!(
            frozen
                .read(pkg_id(&pkg, "value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"first value"
        );

        fs::remove_file(&pkg_link).unwrap();
        std::os::unix::fs::symlink(&second_dir, &pkg_link).unwrap();

        let frozen_again = frozen.read(pkg_id(&pkg, "value.txt"));
        assert_eq!(frozen_again.result.unwrap().as_ref(), b"first value");
        assert_eq!(
            frozen_again.package_checks[0].canonical_target(),
            Some(first_dir.canonicalize().unwrap().as_path())
        );
        assert_eq!(
            file_view(dir.path(), files)
                .read(pkg_id(&pkg, "value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"second value"
        );
    }
}
