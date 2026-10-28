//! Explicit and first-observed compiler files.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::{FileRead, FileResolver, Loaded, ReadAttempt};
use crate::world::ExplicitSources;
use crate::world::package::{PackageCheck, PackageSpec, PackageStore};

type ObservationCell<V> = Arc<OnceLock<V>>;
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
    Arc::clone(
        cells
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceLock::new())),
    )
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
    files: Arc<FileResolver>,
    observations: RwLock<FxHashMap<FileId, FileObservation>>,
    packages: PackageObservations,
}

impl FileView {
    /// Retain explicit sources and the resolver's first observations.
    ///
    /// Sources captured under different restrictions are read again through the resolver.
    pub fn new(sources: Arc<ExplicitSources>, files: Arc<FileResolver>) -> Self {
        let same_boundary = sources.source_boundary() == files.source_boundary();
        let mut view = Self::from_resolver(sources.root(), files);
        view.sources = same_boundary.then_some(sources);
        view
    }

    /// Normalized compilation root owned by this file view.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn from_resolver(root: &Path, files: Arc<FileResolver>) -> Self {
        Self {
            root: root.to_path_buf(),
            sources: None,
            files,
            observations: RwLock::new(FxHashMap::default()),
            packages: PackageObservations::default(),
        }
    }

    pub(crate) fn resolver(&self) -> &Arc<FileResolver> {
        &self.files
    }

    pub(crate) fn source(&self, id: FileId) -> Option<Loaded<Source>> {
        self.sources.as_ref()?.get_source(id)
    }

    pub(crate) fn file(&self, id: FileId) -> Option<Loaded<Bytes>> {
        self.sources.as_ref()?.get_file(id)
    }

    pub(crate) fn read(&self, id: FileId) -> ReadAttempt<Bytes> {
        if let Some(bytes) = self.file(id) {
            return ReadAttempt {
                result: Ok(bytes.value),
                reads: Some(bytes.read),
                disk_reads: Vec::new(),
                package_checks: Vec::new(),
            };
        }
        observation_cell(&self.observations, &id)
            .get_or_init(|| {
                let read = self
                    .files
                    .read_attempt_with_packages(id, self.root(), &self.packages);
                ReadAttempt {
                    result: read.result.map(Bytes::new),
                    reads: read.reads,
                    disk_reads: read.disk_reads,
                    package_checks: read.package_checks,
                }
            })
            .clone()
    }

    /// Observe a file and compare its exact successful read identity.
    pub fn matches_read(&self, id: FileId, expected: &FileRead) -> bool {
        self.matching_accessed(id, expected).is_some()
    }

    /// Observe a file and return its inputs when the successful identity matches.
    pub fn matching_accessed(
        &self,
        id: FileId,
        expected: &FileRead,
    ) -> Option<crate::session::AccessedDeps> {
        let observed = self.read(id);
        (observed.result.is_ok() && observed.reads.as_ref() == Some(expected)).then_some(
            crate::session::AccessedDeps {
                reads: observed.reads.into_iter().collect(),
                disk_reads: observed.disk_reads,
                package_checks: observed.package_checks,
            },
        )
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
        package: &PackageSpec,
        store: &PackageStore,
    ) -> PackageObservation {
        observation_cell(&self.observations, package)
            .get_or_init(|| match store.prepare(package) {
                Ok(prepared) => {
                    let (directory, checks) = prepared.into_directory_and_checks();
                    PackageObservation {
                        directory: Ok(directory),
                        checks,
                    }
                }
                Err(failure) => {
                    let (error, checks) = failure.into_error_and_checks();
                    PackageObservation {
                        directory: Err(error),
                        checks,
                    }
                }
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

    use super::*;

    fn id(path: &str) -> FileId {
        FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(path).unwrap(),
        ))
    }

    fn file_view(root: &Path, files: Arc<FileResolver>) -> FileView {
        let sources = ExplicitSources::build(&[], root)
            .unwrap()
            .into_sources_and_accessed()
            .0;
        FileView::new(Arc::new(sources), files)
    }

    #[test]
    fn file_view_freezes_first_observation() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("value.txt");
        let files = Arc::new(FileResolver::new());

        fs::write(&path, "first").unwrap();
        let present = file_view(directory.path(), Arc::clone(&files));
        assert_eq!(
            present.read(id("value.txt")).result.unwrap().as_ref(),
            b"first"
        );
        fs::write(&path, "second").unwrap();
        assert_eq!(
            present.read(id("value.txt")).result.unwrap().as_ref(),
            b"first"
        );
        assert_eq!(
            file_view(directory.path(), Arc::clone(&files))
                .read(id("value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"second"
        );

        fs::remove_file(&path).unwrap();
        let absent = file_view(directory.path(), Arc::clone(&files));
        assert!(absent.read(id("value.txt")).result.is_err());
        fs::write(&path, "created").unwrap();
        assert!(absent.read(id("value.txt")).result.is_err());
        assert_eq!(
            file_view(directory.path(), files)
                .read(id("value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"created"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_package_symlink_ignored() {
        use typst::syntax::package::PackageSpec;

        fn package_id(package: &PackageSpec, path: &str) -> FileId {
            FileId::new(RootedPath::new(
                VirtualRoot::Package(package.clone()),
                VirtualPath::new(path).unwrap(),
            ))
        }

        let directory = TempDir::new().unwrap();
        let packages = directory.path().join("packages");
        let package_link = packages.join("local/demo/1.0.0");
        let first_directory = directory.path().join("first");
        let second_directory = directory.path().join("second");
        fs::create_dir_all(package_link.parent().unwrap()).unwrap();
        fs::create_dir_all(&first_directory).unwrap();
        fs::create_dir_all(&second_directory).unwrap();
        fs::write(first_directory.join("value.txt"), "first value").unwrap();
        fs::write(second_directory.join("value.txt"), "second value").unwrap();
        std::os::unix::fs::symlink(&first_directory, &package_link).unwrap();
        let locations =
            crate::world::package::PackageLocations::from_absolute_roots(Some(packages), None)
                .unwrap();
        let files = Arc::new(FileResolver::from_package_locations(
            locations,
            crate::world::package::PackageFetchPolicy::LocalOnly,
        ));
        let package: PackageSpec = "@local/demo:1.0.0".parse().unwrap();
        let frozen = file_view(directory.path(), Arc::clone(&files));
        assert_eq!(
            frozen
                .read(package_id(&package, "value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"first value"
        );

        fs::remove_file(&package_link).unwrap();
        std::os::unix::fs::symlink(&second_directory, &package_link).unwrap();

        let frozen_again = frozen.read(package_id(&package, "value.txt"));
        assert_eq!(frozen_again.result.unwrap().as_ref(), b"first value");
        assert_eq!(
            frozen_again.package_checks[0].canonical_target(),
            Some(first_directory.canonicalize().unwrap().as_path())
        );
        assert_eq!(
            file_view(directory.path(), files)
                .read(package_id(&package, "value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"second value"
        );
    }
}
