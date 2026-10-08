//! Explicit file resolution for Typst worlds.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use typst::diag::{FileError, FileResult};
use typst::syntax::FileId;

use super::evidence::{DiskReadPath, FileRead, ReadAttempt, ReadEvidence, ReadLocator, ReadOrigin};
use super::provider::{EmptyFiles, FileProvider, FileTarget};
use super::read::{EMPTY_ID, STDIN_ID, decode_utf8, non_persistent_label, read_disk};
use super::snapshot::PackageSnapshot;
use crate::world::package::{PackageCheck, PackageFetchPolicy, PackageLocations, PackageStore};

/// Resolves Typst file IDs against provided bytes, mapped files, packages, and disk.
#[derive(Clone)]
pub struct FileResolver {
    provider: Arc<dyn FileProvider>,
    packages: PackageStore,
    disk_overrides: Arc<FxHashMap<PathBuf, Arc<[u8]>>>,
    boundary: crate::world::SourceBoundary,
}

impl FileResolver {
    /// Create a resolver with no provided files and package roots frozen from
    /// the environment.
    ///
    /// The default package store reads local roots only. Build one with
    /// `PackageFetchPolicy::AllowNetwork` (the `network` feature) and pass it to
    /// [`Self::from_package_store`] when missing packages may be downloaded.
    pub fn new() -> Self {
        Self::from_package_store(PackageStore::default())
    }

    /// Create a resolver from an already configured package store.
    pub fn from_package_store(packages: PackageStore) -> Self {
        Self {
            provider: Arc::new(EmptyFiles),
            packages,
            disk_overrides: Arc::new(FxHashMap::default()),
            boundary: crate::world::SourceBoundary::default(),
        }
    }

    /// Create a resolver from frozen package roots and an explicit network policy.
    pub fn from_package_locations(
        locations: PackageLocations,
        fetch_policy: PackageFetchPolicy,
    ) -> Self {
        Self::from_package_store(PackageStore::new(locations, fetch_policy))
    }

    /// Apply physical restrictions to disk reads and disk-backed editor overrides.
    pub fn with_source_boundary(mut self, boundary: crate::world::SourceBoundary) -> Self {
        self.packages = self.packages.with_source_boundary(boundary.clone());
        self.boundary = boundary;
        self
    }

    pub(crate) fn source_boundary(&self) -> &crate::world::SourceBoundary {
        &self.boundary
    }

    /// Use a pure provider for root and package file identities.
    pub fn with_provider<P>(self, provider: P) -> Self
    where
        P: FileProvider + 'static,
    {
        Self {
            provider: Arc::new(provider),
            packages: self.packages,
            disk_overrides: self.disk_overrides,
            boundary: self.boundary,
        }
    }

    /// Use immutable bytes for selected physical files without changing file
    /// or package selection.
    ///
    /// Paths must be absolute. Existing prefixes are canonicalized when this
    /// snapshot is constructed, including for files that do not exist yet.
    /// Aliases with conflicting bytes are rejected. The same normalization is
    /// applied after ordinary root, mapped-disk, or package resolution.
    /// A provider's explicit [`FileTarget::Bytes`] takes precedence.
    ///
    /// Overrides are recorded as provider reads, retaining package checks.
    /// When bytes change, create another resolver and use it to construct or
    /// refresh source snapshots; existing snapshots keep their original bytes.
    pub fn with_disk_overrides(
        mut self,
        sources: impl IntoIterator<Item = (PathBuf, Arc<[u8]>)>,
    ) -> FileResult<Self> {
        let mut overrides = FxHashMap::default();
        for (path, bytes) in sources {
            if !path.is_absolute() {
                return Err(FileError::AccessDenied);
            }
            self.boundary.check(&path)?;
            let path = crate::world::normalize_path(&path);
            if let Some(previous) = overrides.get(&path) {
                if previous != &bytes {
                    return Err(FileError::Other(Some(
                        "could not read this source; two different versions were provided".into(),
                    )));
                }
            } else {
                overrides.insert(path, bytes);
            }
        }
        self.disk_overrides = Arc::new(overrides);
        Ok(self)
    }

    fn overridden_bytes(&self, path: &Path) -> Option<&Arc<[u8]>> {
        if self.disk_overrides.is_empty() || !path.is_absolute() {
            return None;
        }
        let path = crate::world::normalize_path(path);
        self.disk_overrides.get(&path)
    }

    /// Read a file ID into an owned byte buffer using this resolver.
    pub fn read(&self, id: FileId, root: &Path) -> FileResult<Vec<u8>> {
        self.read_attempt(id, root)
            .result
            .map(|bytes| bytes.as_ref().to_vec())
    }

    /// Read the whole text a caller's path names, the reverse of `reported_path`.
    ///
    /// A site file is read below `root`; a package file is read from the package the
    /// author imported. A renderer needs this to show the source a diagnostic points into.
    pub fn read_reported(&self, root: &Path, reported: &Path) -> FileResult<String> {
        let name = reported.to_string_lossy();
        let bytes = match name.strip_prefix('@') {
            // `@namespace/name:version`, then the path inside the package.
            Some(package) => {
                let mut segments = package.splitn(3, '/');
                let namespace = segments.next().unwrap_or_default();
                let name = segments
                    .next()
                    .ok_or_else(|| FileError::NotFound(reported.to_path_buf()))?;
                let inner = segments
                    .next()
                    .ok_or_else(|| FileError::NotFound(reported.to_path_buf()))?;
                let spec = format!("@{namespace}/{name}");
                let prepared = self
                    .packages
                    .prepare_package(spec)
                    .map_err(|failure| failure.into_error_and_checks().0)?;
                let path = prepared.directory().join(inner);
                self.boundary.check(&path)?;
                read_disk(&path, reported)?
            }
            None => {
                let path = root.join(reported);
                self.boundary.check(&path)?;
                read_disk(&path, reported)?
            }
        };
        Ok(decode_utf8(&bytes)?.to_owned())
    }

    pub(crate) fn read_attempt(&self, id: FileId, root: &Path) -> ReadAttempt<Arc<[u8]>> {
        self.read_attempt_inner(id, root, None)
    }

    pub(crate) fn read_attempt_with_snapshot(
        &self,
        id: FileId,
        root: &Path,
        packages: &PackageSnapshot,
    ) -> ReadAttempt<Arc<[u8]>> {
        self.read_attempt_inner(id, root, Some(packages))
    }

    fn read_attempt_inner(
        &self,
        id: FileId,
        root: &Path,
        packages: Option<&PackageSnapshot>,
    ) -> ReadAttempt<Arc<[u8]>> {
        if id == *EMPTY_ID {
            return successful_provider_read(
                Arc::from(&b""[..]),
                ReadLocator::NonPersistent("<empty>".to_owned()),
                ReadOrigin::NonPersistent,
                Vec::new(),
            );
        }
        if id == *STDIN_ID {
            return match read_stdin() {
                Ok(bytes) => successful_provider_read(
                    bytes.into(),
                    ReadLocator::NonPersistent("<stdin>".to_owned()),
                    ReadOrigin::NonPersistent,
                    Vec::new(),
                ),
                Err(error) => ReadAttempt::without_inputs(Err(error)),
            };
        }

        let non_persistent = non_persistent_label(id);

        if let Some(target) = self.provider.target(id) {
            return match target {
                FileTarget::Bytes(content) => successful_provider_read(
                    content,
                    locator_for(id, non_persistent, true),
                    ReadOrigin::Provider,
                    Vec::new(),
                ),
                FileTarget::Disk(path) => {
                    self.read_disk_target(id, non_persistent, path, true, Vec::new())
                }
                FileTarget::Missing => {
                    ReadAttempt::without_inputs(Err(FileError::NotFound(reported_path(id))))
                }
            };
        }

        // A namespace the provider owns is served only from the provider, so a package
        // directory in it neither shadows nor extends what the provider supplies.
        if let typst::syntax::VirtualRoot::Package(spec) = id.root()
            && self
                .provider
                .owned_namespaces()
                .contains(&spec.namespace.as_str())
        {
            return ReadAttempt::without_inputs(Err(FileError::Package(
                typst::diag::PackageError::NotFound(spec.clone()),
            )));
        }

        let (path, package_checks) = match self.resolve_path(root, id, packages) {
            Ok(resolved) => resolved,
            Err((error, package_checks)) => {
                return failed_resolution(error, package_checks);
            }
        };
        self.read_disk_target(id, non_persistent, path, false, package_checks)
    }

    fn read_disk_target(
        &self,
        id: FileId,
        non_persistent: Option<String>,
        path: PathBuf,
        provider_selected: bool,
        package_checks: Vec<PackageCheck>,
    ) -> ReadAttempt<Arc<[u8]>> {
        if provider_selected && !path.is_absolute() {
            return failed_resolution(FileError::AccessDenied, package_checks);
        }
        let path = if path.is_absolute() {
            path
        } else {
            match std::path::absolute(path) {
                Ok(path) => path,
                Err(error) => {
                    return failed_resolution(
                        FileError::from_io(error, &reported_path(id)),
                        package_checks,
                    );
                }
            }
        };
        if let Err(error) = self.boundary.check(&path) {
            return failed_resolution(error, package_checks);
        }
        if let Some(bytes) = self.overridden_bytes(&path) {
            return successful_provider_read(
                Arc::clone(bytes),
                locator_for(id, non_persistent, true),
                ReadOrigin::Provider,
                package_checks,
            );
        }
        let disk_read = match DiskReadPath::new(path) {
            Ok(path) => path,
            Err(error) => {
                return failed_resolution(error, package_checks);
            }
        };
        match read_disk(disk_read.as_path(), &reported_path(id)) {
            Ok(bytes) => {
                let evidence =
                    ReadEvidence::new(locator_for(id, non_persistent, provider_selected), &bytes);
                let read = FileRead::new(evidence, ReadOrigin::Disk(disk_read.clone()));
                ReadAttempt {
                    result: Ok(bytes.into()),
                    reads: Some(read),
                    disk_reads: vec![disk_read],
                    package_checks,
                }
            }
            Err(error) => ReadAttempt {
                result: Err(error),
                reads: None,
                disk_reads: vec![disk_read],
                package_checks,
            },
        }
    }

    fn resolve_path(
        &self,
        root: &Path,
        id: FileId,
        package_snapshot: Option<&PackageSnapshot>,
    ) -> Result<(PathBuf, Vec<PackageCheck>), (FileError, Vec<PackageCheck>)> {
        let (root, package_checks) = match id.root() {
            typst::syntax::VirtualRoot::Project => (root.to_path_buf(), Vec::new()),
            typst::syntax::VirtualRoot::Package(spec) => {
                if let Some(snapshot) = package_snapshot {
                    let observation = snapshot.observe(spec, &self.packages);
                    match observation.directory {
                        Ok(directory) => (directory, observation.checks),
                        Err(error) => return Err((error, observation.checks)),
                    }
                } else {
                    match self.packages.prepare(spec) {
                        Ok(prepared) => prepared.into_directory_and_checks(),
                        Err(failure) => {
                            let (error, checks) = failure.into_error_and_checks();
                            return Err((error, checks));
                        }
                    }
                }
            }
        };

        match id.vpath().realize(&root) {
            Ok(path) => Ok((path, package_checks)),
            Err(_) => Err((FileError::AccessDenied, package_checks)),
        }
    }
}

/// Resolution can observe package selection without attempting a disk read.
fn failed_resolution(
    error: FileError,
    package_checks: Vec<PackageCheck>,
) -> ReadAttempt<Arc<[u8]>> {
    ReadAttempt {
        result: Err(error),
        reads: None,
        disk_reads: Vec::new(),
        package_checks,
    }
}

fn successful_provider_read(
    bytes: Arc<[u8]>,
    locator: ReadLocator,
    origin: ReadOrigin,
    package_checks: Vec<PackageCheck>,
) -> ReadAttempt<Arc<[u8]>> {
    let evidence = ReadEvidence::new(locator, &bytes);
    let read = FileRead::new(evidence, origin);
    ReadAttempt {
        result: Ok(bytes),
        reads: Some(read),
        disk_reads: Vec::new(),
        package_checks,
    }
}

fn locator_for(id: FileId, non_persistent: Option<String>, is_virtual: bool) -> ReadLocator {
    if let Some(label) = non_persistent {
        return ReadLocator::NonPersistent(label);
    }

    let path = virtual_relative_path(id);
    match (id.root(), is_virtual) {
        (typst::syntax::VirtualRoot::Project, false) => ReadLocator::Root(path),
        (typst::syntax::VirtualRoot::Project, true) => ReadLocator::ProvidedRoot(path),
        (typst::syntax::VirtualRoot::Package(spec), false) => ReadLocator::Package {
            package: spec.clone(),
            path,
        },
        (typst::syntax::VirtualRoot::Package(spec), true) => ReadLocator::ProvidedPackage {
            package: spec.clone(),
            path,
        },
    }
}

fn virtual_relative_path(id: FileId) -> PathBuf {
    PathBuf::from(id.vpath().get_with_slash().trim_start_matches('/'))
}

/// The path a caller recognizes for one file ID, never a host path.
///
/// Site files keep their compilation-relative spelling; package files are named by
/// the import identity the author wrote.
fn reported_path(id: FileId) -> PathBuf {
    let path = virtual_relative_path(id);
    match id.root() {
        typst::syntax::VirtualRoot::Project => path,
        typst::syntax::VirtualRoot::Package(spec) => PathBuf::from(spec.to_string()).join(path),
    }
}

impl Default for FileResolver {
    fn default() -> Self {
        Self::new()
    }
}

fn read_stdin() -> FileResult<Vec<u8>> {
    let mut buf = Vec::new();
    io::stdin().read_to_end(&mut buf).or_else(|e| {
        if e.kind() == io::ErrorKind::BrokenPipe {
            Ok(0)
        } else {
            Err(FileError::from_io(e, Path::new("<stdin>")))
        }
    })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use typst::syntax::{FileId, VirtualPath};

    struct DiskMapping {
        id: FileId,
        path: PathBuf,
    }

    impl FileProvider for DiskMapping {
        fn target(&self, id: FileId) -> Option<FileTarget> {
            (id == self.id).then(|| FileTarget::Disk(self.path.clone()))
        }
    }

    fn package_file_id(spec: &str, path: &str) -> FileId {
        FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package(spec.parse().unwrap()),
            VirtualPath::new(path).unwrap(),
        ))
    }

    #[cfg(unix)]
    #[test]
    fn package_members_respect_containment() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("site");
        let package_root = root.join("vendor/local/demo/1.0.0");
        fs::create_dir_all(&package_root).unwrap();
        let outside = directory.path().join("outside.typ");
        fs::write(&outside, "host source").unwrap();
        let inside = root.join("inside.typ");
        fs::write(&inside, "site source").unwrap();
        let locations = PackageLocations::default()
            .with_declared_root(root.join("vendor"))
            .unwrap();
        let files = FileResolver::from_package_locations(locations, PackageFetchPolicy::LocalOnly)
            .with_source_boundary(crate::SourceBoundary::new(&root, true));
        for (name, target, permitted) in [
            ("outside.typ", &outside, false),
            ("inside.typ", &inside, true),
        ] {
            std::os::unix::fs::symlink(target, package_root.join(name)).unwrap();
            let id = package_file_id("@local/demo:1.0.0", name);
            let observed = files.read_attempt(id, &root);
            assert!(
                observed
                    .package_checks
                    .iter()
                    .any(PackageCheck::was_selected)
            );
            if permitted {
                assert_eq!(observed.result.unwrap().as_ref(), b"site source");
            } else {
                assert!(observed.result.is_err());
                assert!(observed.reads.is_none());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn generated_aliases_are_not_sources() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let internal = root.join(".tola");
        fs::create_dir(&internal).unwrap();
        fs::write(internal.join("cached.typ"), "cache source").unwrap();
        std::os::unix::fs::symlink(internal.join("cached.typ"), root.join("alias.typ")).unwrap();
        let files = FileResolver::new()
            .with_source_boundary(crate::SourceBoundary::new(root, false).excluding(internal));
        for name in [".tola/cached.typ", "alias.typ"] {
            assert!(files.read(crate::file_id(name), root).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn unsaved_aliases_keep_source_limits() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("site");
        let outside = directory.path().join("outside");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("alias")).unwrap();
        let files =
            FileResolver::new().with_source_boundary(crate::SourceBoundary::new(&root, true));
        for exists in [false, true] {
            if exists {
                fs::create_dir(&outside).unwrap();
            }
            assert!(
                files
                    .clone()
                    .with_disk_overrides([(
                        root.join("alias/unsaved.typ"),
                        Arc::from(&b"unsaved host source"[..])
                    ),])
                    .is_err()
            );
        }
    }

    #[test]
    fn disk_overrides_precede_package_disk() {
        let directory = TempDir::new().unwrap();
        let data = directory.path().join("data");
        let cache = directory.path().join("cache");
        let selected = data.join("local/demo/1.0.0/lib.typ");
        let shadowed = cache.join("local/demo/1.0.0/lib.typ");
        for path in [&selected, &shadowed] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "disk source").unwrap();
        }
        let locations = PackageLocations::from_absolute_roots(Some(data), Some(cache)).unwrap();
        let id = package_file_id("@local/demo:1.0.0", "lib.typ");
        let resolver =
            FileResolver::from_package_locations(locations, PackageFetchPolicy::LocalOnly)
                .with_disk_overrides([(shadowed, Arc::from(&b"shadowed editor source"[..]))])
                .unwrap();
        assert_eq!(resolver.read(id, directory.path()).unwrap(), b"disk source");

        let resolver = resolver
            .with_disk_overrides([(selected.clone(), Arc::from(&b"editor source"[..]))])
            .unwrap();
        let observed = resolver.read_attempt(id, directory.path());
        assert_eq!(observed.result.unwrap().as_ref(), b"editor source");
        assert!(observed.disk_reads.is_empty());
        assert_eq!(observed.package_checks.len(), 1);
        assert!(observed.package_checks[0].was_selected());
        let read = observed
            .reads
            .as_ref()
            .expect("a provider read has evidence");
        assert!(matches!(read.origin(), ReadOrigin::Provider));
        assert!(matches!(
            read.evidence().locator(),
            ReadLocator::ProvidedPackage { .. }
        ));
        assert_eq!(fs::read(selected).unwrap(), b"disk source");
    }

    #[test]
    fn provider_bytes_outrank_disk_overrides() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("lib.typ");
        fs::write(&path, "disk").unwrap();
        let id = crate::world::file::file_id("lib.typ");
        let mut provided = crate::world::file::FileMap::new();
        provided.insert(id, &b"virtual source"[..]);
        let resolver = FileResolver::new()
            .with_provider(provided)
            .with_disk_overrides([(path, Arc::from(&b"editor source"[..]))])
            .unwrap();
        assert_eq!(
            resolver.read(id, directory.path()).unwrap(),
            b"virtual source"
        );
    }

    #[cfg(unix)]
    #[test]
    fn conflicting_disk_override_aliases_fail() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("source.typ");
        let alias = directory.path().join("alias.typ");
        fs::write(&path, "disk").unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let resolver = FileResolver::new()
            .with_disk_overrides([(path.clone(), Arc::from(&b"editor source"[..]))])
            .unwrap();
        assert_eq!(
            resolver
                .read(crate::world::file::file_id("alias.typ"), directory.path())
                .unwrap(),
            b"editor source"
        );
        assert!(
            FileResolver::new()
                .with_disk_overrides([
                    (path, Arc::from(&b"first"[..])),
                    (alias, Arc::from(&b"second"[..])),
                ])
                .is_err()
        );
    }

    #[test]
    fn package_read_reports_its_checks() {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        let package_dir = cache.join("preview/demo/0.1.0");
        fs::create_dir_all(&package_dir).unwrap();
        let package_file = package_dir.join("lib.typ");
        fs::write(&package_file, "#let value = 1").unwrap();
        let locations =
            PackageLocations::from_absolute_roots(Some(data.clone()), Some(cache)).unwrap();
        let files = FileResolver::from_package_locations(locations, PackageFetchPolicy::LocalOnly);
        let id = package_file_id("@preview/demo:0.1.0", "lib.typ");

        let attempt = files.read_attempt(id, dir.path());

        assert!(attempt.result.is_ok());
        assert_eq!(attempt.package_checks.len(), 2);
        assert_eq!(
            attempt.package_checks[0].candidate(),
            data.join("preview/demo/0.1.0")
        );
        assert!(!attempt.package_checks[0].was_selected());
        assert!(attempt.package_checks[1].was_selected());
        assert_eq!(attempt.disk_reads.len(), 1);
        assert_eq!(
            attempt.disk_reads[0].as_path(),
            package_file.canonicalize().unwrap()
        );
    }

    #[test]
    fn read_attempt_reports_disk_inputs() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("page.typ"), "= Page").unwrap();
        let files = FileResolver::new();

        let attempt = files.read_attempt(crate::world::file::file_id("page.typ"), dir.path());
        let path = dir.path().join("page.typ");
        assert!(attempt.result.is_ok());
        assert_eq!(attempt.disk_reads.len(), 1);
        assert_eq!(attempt.disk_reads[0].as_path(), path);
        let read = attempt
            .reads
            .as_ref()
            .expect("a successful read has evidence");
        assert!(matches!(
            read.origin(),
            ReadOrigin::Disk(origin) if origin.as_path() == path
        ));

        let attempt = files.read_attempt(crate::world::file::file_id("missing.typ"), dir.path());
        assert!(attempt.result.is_err());
        assert!(attempt.reads.is_none());
        assert_eq!(attempt.disk_reads.len(), 1);
        assert_eq!(
            attempt.disk_reads[0].as_path(),
            dir.path().join("missing.typ")
        );
    }

    #[test]
    fn provided_bytes_create_no_disk_read() {
        let directory = TempDir::new().unwrap();
        let id = crate::world::file::file_id("provided.typ");
        let mut provider = crate::world::file::FileMap::new();
        provider.insert(id, Arc::<[u8]>::from(&b"= Provided"[..]));

        let attempt = FileResolver::new()
            .with_provider(provider)
            .read_attempt(id, directory.path());

        assert_eq!(attempt.result.unwrap().as_ref(), b"= Provided");
        assert!(attempt.disk_reads.is_empty());
        assert!(matches!(
            attempt
                .reads
                .as_ref()
                .expect("a provider read has evidence")
                .origin(),
            ReadOrigin::Provider
        ));
    }

    #[test]
    fn provider_absence_stops_resolution() {
        struct AbsentFiles;
        impl FileProvider for AbsentFiles {
            fn target(&self, _: FileId) -> Option<FileTarget> {
                Some(FileTarget::Missing)
            }
        }

        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("image.svg"), b"unrelated disk image").unwrap();
        let resolver = FileResolver::from_package_locations(
            PackageLocations::from_absolute_roots(None, None).unwrap(),
            PackageFetchPolicy::LocalOnly,
        )
        .with_provider(AbsentFiles);
        let root = crate::world::file::file_id("image.svg");
        let package = package_file_id("@local/icons:1.0.0", "image.svg");
        for id in [root, package] {
            let observed = resolver.read_attempt(id, directory.path());
            assert!(matches!(observed.result, Err(FileError::NotFound(_))));
            assert!(observed.disk_reads.is_empty());
            assert!(observed.package_checks.is_empty());
        }
    }

    #[test]
    fn mapped_disk_failure_reads_no_fallback() {
        let directory = TempDir::new().unwrap();
        let logical = directory.path().join("image.svg");
        fs::write(&logical, b"wrong fallback").unwrap();
        let mapped = directory.path().join("assets/missing.svg");
        let id = crate::world::file::file_id("image.svg");
        let files = FileResolver::new().with_provider(DiskMapping {
            id,
            path: mapped.clone(),
        });

        let attempt = files.read_attempt(id, directory.path());

        assert!(attempt.result.is_err());
        assert!(attempt.reads.is_none());
        assert_eq!(attempt.disk_reads.len(), 1);
        assert_eq!(attempt.disk_reads[0].as_path(), mapped);
    }

    #[test]
    fn mapped_disk_targets_must_be_absolute() {
        let directory = TempDir::new().unwrap();
        let logical = directory.path().join("image.svg");
        fs::write(&logical, b"root file").unwrap();
        let id = crate::world::file::file_id("image.svg");
        let files = FileResolver::new().with_provider(DiskMapping {
            id,
            path: PathBuf::from("relative/image.svg"),
        });

        let attempt = files.read_attempt(id, directory.path());

        assert!(attempt.result.is_err());
        assert!(attempt.reads.is_none());
        assert!(attempt.disk_reads.is_empty());
    }
}
