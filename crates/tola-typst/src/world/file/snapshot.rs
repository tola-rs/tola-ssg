//! Explicit and first-observed compiler files.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::{FileRead, FileResolver, Loaded, ReadAttempt};
use crate::world::SourceSnapshot;
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
/// Explicit source bytes come from the supplied [`SourceSnapshot`]. Every
/// other successful read, failure, and package selection is retained from its
/// first observation. Create another snapshot to observe changed files.
/// Different files can be first observed at different times.
pub struct FileSnapshot {
    root: PathBuf,
    sources: Option<Arc<SourceSnapshot>>,
    files: Arc<FileResolver>,
    observations: RwLock<FxHashMap<FileId, FileObservation>>,
    packages: PackageSnapshot,
}

impl FileSnapshot {
    /// Retain explicit sources and the resolver's first observations.
    ///
    /// Sources captured under different restrictions are read again through the resolver.
    pub fn new(sources: Arc<SourceSnapshot>, files: Arc<FileResolver>) -> Self {
        let same_boundary = sources.source_boundary() == files.source_boundary();
        let mut snapshot = Self::from_resolver(sources.root(), files);
        snapshot.sources = same_boundary.then_some(sources);
        snapshot
    }

    /// Normalized compilation root owned by this snapshot view.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn from_resolver(root: &Path, files: Arc<FileResolver>) -> Self {
        Self {
            root: root.to_path_buf(),
            sources: None,
            files,
            observations: RwLock::new(FxHashMap::default()),
            packages: PackageSnapshot::default(),
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
                    .read_attempt_with_snapshot(id, self.root(), &self.packages);
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
pub(crate) struct PackageSnapshot {
    observations: RwLock<FxHashMap<PackageSpec, PackageObservationCell>>,
}

#[derive(Clone)]
pub(crate) struct PackageObservation {
    pub(crate) directory: typst::diag::FileResult<PathBuf>,
    pub(crate) checks: Vec<PackageCheck>,
}

impl PackageSnapshot {
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
    use typst::syntax::package::PackageSpec;
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

    use super::*;

    fn id(path: &str) -> FileId {
        FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(path).unwrap(),
        ))
    }

    fn package_id(package: &PackageSpec, path: &str) -> FileId {
        FileId::new(RootedPath::new(
            VirtualRoot::Package(package.clone()),
            VirtualPath::new(path).unwrap(),
        ))
    }

    fn file_snapshot(root: &Path, files: Arc<FileResolver>) -> FileSnapshot {
        let snapshot = SourceSnapshot::build(&[], root)
            .unwrap()
            .into_snapshot_and_accessed()
            .0;
        FileSnapshot::new(Arc::new(snapshot), files)
    }

    #[test]
    fn snapshot_freezes_first_observation() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("value.txt");
        let files = Arc::new(FileResolver::new());

        fs::write(&path, "first").unwrap();
        let present = file_snapshot(directory.path(), Arc::clone(&files));
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
            file_snapshot(directory.path(), Arc::clone(&files))
                .read(id("value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"second"
        );

        fs::remove_file(&path).unwrap();
        let absent = file_snapshot(directory.path(), Arc::clone(&files));
        assert!(absent.read(id("value.txt")).result.is_err());
        fs::write(&path, "created").unwrap();
        assert!(absent.read(id("value.txt")).result.is_err());
        assert_eq!(
            file_snapshot(directory.path(), files)
                .read(id("value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"created"
        );
    }

    #[test]
    fn repeated_reads_share_bytes() {
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("shared.bin"), vec![42; 1024 * 1024]).unwrap();
        let snapshot = file_snapshot(directory.path(), Arc::new(FileResolver::new()));
        let first = snapshot.read(id("shared.bin"));
        let expected = first.reads.clone().expect("a successful read has evidence");
        let bytes = first.result.unwrap();

        assert!(snapshot.matches_read(id("shared.bin"), &expected));
        let repeated = snapshot.read(id("shared.bin")).result.unwrap();
        assert!(std::ptr::eq(
            bytes.as_slice().as_ptr(),
            repeated.as_slice().as_ptr()
        ));
    }

    #[test]
    fn snapshot_keeps_package_selection() {
        let directory = TempDir::new().unwrap();
        let data = directory.path().join("data");
        let cache = directory.path().join("cache");
        let cached_package = cache.join("preview/demo/1.0.0");
        fs::create_dir_all(&cached_package).unwrap();
        fs::write(cached_package.join("lib.typ"), "cache lib").unwrap();
        fs::write(cached_package.join("other.txt"), "cache other").unwrap();
        let locations = crate::world::package::PackageLocations::from_absolute_roots(
            Some(data.clone()),
            Some(cache),
        )
        .unwrap();
        let files = Arc::new(FileResolver::from_package_locations(
            locations,
            crate::world::package::PackageFetchPolicy::LocalOnly,
        ));
        let package: PackageSpec = "@preview/demo:1.0.0".parse().unwrap();
        let first = file_snapshot(directory.path(), Arc::clone(&files));

        assert_eq!(
            first
                .read(package_id(&package, "lib.typ"))
                .result
                .unwrap()
                .as_ref(),
            b"cache lib"
        );
        let data_package = data.join("preview/demo/1.0.0");
        fs::create_dir_all(&data_package).unwrap();
        fs::write(data_package.join("other.txt"), "data other").unwrap();
        assert_eq!(
            first
                .read(package_id(&package, "other.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"cache other"
        );

        let second = file_snapshot(directory.path(), files);
        assert_eq!(
            second
                .read(package_id(&package, "other.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"data other"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_package_symlink_ignored() {
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
        let frozen = file_snapshot(directory.path(), Arc::clone(&files));
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
            file_snapshot(directory.path(), files)
                .read(package_id(&package, "value.txt"))
                .result
                .unwrap()
                .as_ref(),
            b"second value"
        );
    }

    #[test]
    fn frozen_source_matches_file_bytes() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("main.typ");
        let original = b"\xef\xbb\xbf= Frozen";
        fs::write(&path, original).unwrap();
        let files = Arc::new(FileResolver::new());
        let loaded = SourceSnapshot::build_with_files(
            std::slice::from_ref(&path),
            directory.path(),
            Arc::clone(&files),
        )
        .unwrap();
        let (snapshot, accessed) = loaded.into_snapshot_and_accessed();
        let expected = accessed.reads.first().unwrap().clone();
        let snapshot = FileSnapshot::new(Arc::new(snapshot), files);
        fs::write(&path, "= Changed").unwrap();

        assert_eq!(
            snapshot.source(id("main.typ")).unwrap().value.text(),
            "= Frozen"
        );
        assert_eq!(
            snapshot.file(id("main.typ")).unwrap().value.as_slice(),
            original
        );
        assert!(snapshot.matches_read(id("main.typ"), &expected));
    }

    #[test]
    fn world_reads_overridden_source() {
        use typst::World;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("main.typ");
        let added = directory.path().join("added.typ");
        fs::write(&path, "disk source").unwrap();
        let files = Arc::new(
            FileResolver::new()
                .with_disk_overrides([
                    (path.clone(), Arc::from(&b"editor source"[..])),
                    (added.clone(), Arc::from(&b"new source"[..])),
                ])
                .unwrap(),
        );
        let snapshot = Arc::new(
            SourceSnapshot::build_with_files(
                &[path.clone(), added],
                directory.path(),
                Arc::clone(&files),
            )
            .unwrap()
            .into_snapshot_and_accessed()
            .0,
        );
        let snapshot = Arc::new(FileSnapshot::new(snapshot, files));
        fs::write(&path, "changed disk source").unwrap();
        let world = crate::world::TypstWorld::builder(&path, directory.path())
            .with_file_snapshot(
                snapshot.clone(),
                Arc::new(super::super::SharedFileCache::new()),
            )
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();

        assert_eq!(world.source(world.main()).unwrap().text(), "editor source");
        assert_eq!(
            world.file(world.main()).unwrap().as_slice(),
            b"editor source"
        );
        assert_eq!(world.source(id("added.typ")).unwrap().text(), "new source");
        let read = snapshot.source(world.main()).unwrap().read;
        assert!(matches!(
            read.origin(),
            crate::world::file::ReadOrigin::Provider
        ));
        assert!(snapshot.matches_read(world.main(), &read));
    }
}
