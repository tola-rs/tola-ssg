//! Explicit file resolution for Typst worlds.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use typst::diag::{FileError, FileResult};
use typst::syntax::FileId;

use super::local_packages::LocalPackages;
use super::provider::{EmptyFiles, FileProvider, FileTarget};
use super::read::{EMPTY_ID, STDIN_ID, decode_utf8, non_persistent_label, read_disk};
use super::reads::{DiskReadPath, FileRead, ReadAttempt, ReadLocator, ReadOrigin, ReadRecord};
use super::view::PackageObservations;
use crate::world::package::{PackageCheck, PackageFetchMode, PackageLocations, PackageStore};

/// Resolves Typst file IDs against provided bytes, mapped files, packages, and disk.
#[derive(Clone)]
pub struct FileStore {
    provider: Arc<dyn FileProvider>,
    packages: PackageStore,
    local_packages: Arc<LocalPackages>,
    disk_overrides: Arc<FxHashMap<PathBuf, Arc<[u8]>>>,
    boundary: crate::world::SourceBoundary,
}

impl FileStore {
    /// Create a resolver with no provided files and package roots frozen from
    /// the environment.
    ///
    /// The default package store reads local roots only. Build one with
    /// `PackageFetchMode::AllowNetwork` (the `network` feature) and pass it to
    /// [`Self::from_package_store`] when missing packages may be downloaded.
    pub fn new() -> Self {
        Self::from_package_store(PackageStore::default())
    }

    /// Create a resolver from an already configured package store.
    pub fn from_package_store(packages: PackageStore) -> Self {
        Self {
            provider: Arc::new(EmptyFiles),
            packages,
            local_packages: Arc::default(),
            disk_overrides: Arc::new(FxHashMap::default()),
            boundary: crate::world::SourceBoundary::default(),
        }
    }

    /// Create a resolver from frozen package roots and an explicit network policy.
    pub fn from_package_locations(
        locations: PackageLocations,
        fetch_policy: PackageFetchMode,
    ) -> Self {
        Self::from_package_store(PackageStore::new(locations, fetch_policy))
    }

    /// Apply physical restrictions to disk reads and disk-backed editor overrides.
    pub fn with_source_boundary(mut self, boundary: crate::world::SourceBoundary) -> Self {
        self.packages = self.packages.with_source_boundary(boundary.clone());
        self.boundary = boundary;
        self
    }

    /// Require a caller's check of every package root before it is read.
    ///
    /// A refused root fails the read that selected it, so no file below an unverified directory
    /// ever reaches the compiler.
    pub fn with_package_verifier(
        mut self,
        verifier: crate::world::package::PackageVerifier,
    ) -> Self {
        self.packages = self.packages.with_verifier(verifier);
        self
    }

    pub(crate) fn source_boundary(&self) -> &crate::world::SourceBoundary {
        &self.boundary
    }

    pub(crate) fn package_store(&self) -> &PackageStore {
        &self.packages
    }

    /// Use a pure provider for root and package file identities.
    pub fn with_provider<P>(self, provider: P) -> Self
    where
        P: FileProvider + 'static,
    {
        Self {
            provider: Arc::new(provider),
            packages: self.packages,
            local_packages: self.local_packages,
            disk_overrides: self.disk_overrides,
            boundary: self.boundary,
        }
    }

    /// Layer a read-only provider above the current one.
    ///
    /// The overlay is consulted first; the base provider keeps its owned namespaces.
    pub fn with_overlay<P>(self, overlay: P) -> Self
    where
        P: FileProvider + 'static,
    {
        Self {
            provider: Arc::new(OverlayFiles {
                overlay: Arc::new(overlay),
                base: self.provider,
            }),
            ..self
        }
    }

    /// Install declared local packages without replacing the file provider.
    ///
    /// Provider targets and its exclusive namespaces retain precedence. Local
    /// packages are selected before global package discovery; their reserved
    /// namespaces never fall through to host directories or network fetching.
    pub fn with_local_packages(mut self, packages: Arc<LocalPackages>) -> Self {
        self.local_packages = packages;
        self
    }

    /// The immutable local package selection of this resolver.
    pub fn local_packages(&self) -> &Arc<LocalPackages> {
        &self.local_packages
    }

    /// The editable physical file of a declared local package, without reading its contents.
    /// Virtual files, including the generated manifest, have no physical source.
    /// Both the package boundary and the caller's source boundary still apply.
    pub fn local_package_path(&self, fid: FileId) -> FileResult<Option<PathBuf>> {
        let typst::syntax::VirtualRoot::Package(spec) = fid.root() else {
            return Ok(None);
        };
        if self.provider.target(fid).is_some()
            || self
                .provider
                .owned_namespaces()
                .contains(&spec.namespace.as_str())
        {
            return Ok(None);
        }
        let Some(pkg) = self.local_packages.package(spec) else {
            return Ok(None);
        };
        if fid.vpath().get_without_slash() == "typst.toml" {
            return Ok(None);
        }
        let path = fid
            .vpath()
            .realize(pkg.root())
            .map_err(|_| FileError::AccessDenied)?;
        check_pkg_boundary(pkg.boundary(), &path, fid)?;
        self.boundary.check(&path)?;
        Ok(Some(path))
    }

    /// All declared package identities that can address an editor's physical path.
    /// A shared directory may have several aliases; the caller must not pick one arbitrarily.
    pub fn local_package_ids(&self, path: &Path) -> FileResult<Vec<FileId>> {
        self.boundary.check(path)?;
        let mut ids = Vec::new();
        for pkg in self.local_packages.iter() {
            // Preserve the spelling below the package root: an internal symlink's
            // relative imports belong to the path the editor actually opened.
            let prefix = if path.starts_with(pkg.root()) {
                Some(pkg.root())
            } else {
                path.ancestors().find(|ancestor| {
                    ancestor
                        .canonicalize()
                        .is_ok_and(|resolved| resolved == pkg.root())
                })
            };
            let Some(prefix) = prefix else { continue };
            let Ok(rel) = typst::syntax::VirtualPath::virtualize(prefix, path) else {
                continue;
            };
            if rel.is_root() {
                continue;
            }
            let fid = pkg.file_id(rel);
            if self.local_package_path(fid)?.is_some() {
                ids.push(fid);
            }
        }
        ids.sort_by_key(|fid| format!("{:?}", fid.get()));
        Ok(ids)
    }

    /// Use immutable bytes for selected physical files without changing file
    /// or package selection.
    ///
    /// Paths must be absolute. Existing prefixes are canonicalized when this
    /// view is constructed, including for files that do not exist yet.
    /// Aliases with conflicting bytes are rejected. The same normalization is
    /// applied after ordinary root, mapped-disk, or package resolution.
    /// A provider's explicit [`FileTarget::Bytes`] takes precedence.
    ///
    /// Overrides are recorded as provider reads, retaining package checks.
    /// When bytes change, create another resolver and use it to construct or
    /// refresh a file view; an existing view keeps its original bytes.
    pub fn with_disk_overrides(
        mut self,
        sources: impl IntoIterator<Item = (PathBuf, Arc<[u8]>)>,
    ) -> FileResult<Self> {
        let mut overrides = FxHashMap::default();
        for (path, buf) in sources {
            if !path.is_absolute() {
                return Err(FileError::AccessDenied);
            }
            self.boundary.check(&path)?;
            let path = crate::world::normalize_path(&path);
            if let Some(prev) = overrides.get(&path) {
                if prev != &buf {
                    return Err(FileError::Other(Some(
                        "could not read this source; two different versions were provided".into(),
                    )));
                }
            } else {
                overrides.insert(path, buf);
            }
        }
        self.disk_overrides = Arc::new(overrides);
        Ok(self)
    }

    fn overridden_bytes(&self, path: &Path) -> Option<&Arc<[u8]>> {
        if self.disk_overrides.is_empty() || !path.is_absolute() {
            return None;
        }
        self.disk_overrides.get(&crate::world::normalize_path(path))
    }

    /// Read a file ID into an owned byte buffer using this resolver.
    pub fn read(&self, fid: FileId, root: &Path) -> FileResult<Vec<u8>> {
        self.read_attempt(fid, root)
            .result
            .map(|buf| buf.as_ref().to_vec())
    }

    /// Read the whole text a caller's path names, the reverse of `reported_path`.
    ///
    /// A site file is read below `root`; a package file is read from the package the
    /// author imported. A renderer needs this to show the source a diagnostic points into.
    pub fn read_reported(&self, root: &Path, reported: &Path) -> FileResult<String> {
        let name = reported.to_string_lossy();
        let buf = match name.strip_prefix('@') {
            Some(pkg) => {
                let pkg = pkg.replace('\\', "/");
                let mut segs = pkg.splitn(3, '/');
                let ns = segs.next().unwrap_or_default();
                let name = segs
                    .next()
                    .ok_or_else(|| FileError::NotFound(reported.to_path_buf()))?;
                let inner = segs
                    .next()
                    .ok_or_else(|| FileError::NotFound(reported.to_path_buf()))?;
                let spec = format!("@{ns}/{name}")
                    .parse::<typst::syntax::package::PackageSpec>()
                    .map_err(|_| FileError::NotFound(reported.to_path_buf()))?;
                let vpath =
                    typst::syntax::VirtualPath::new(inner).map_err(|_| FileError::AccessDenied)?;
                self.read(
                    FileId::new(typst::syntax::RootedPath::new(
                        typst::syntax::VirtualRoot::Package(spec),
                        vpath,
                    )),
                    root,
                )?
            }
            None => {
                let path = root.join(reported);
                self.boundary.check(&path)?;
                read_disk(&path, reported)?
            }
        };
        Ok(decode_utf8(&buf)?.to_owned())
    }

    pub(crate) fn read_attempt(&self, fid: FileId, root: &Path) -> ReadAttempt<Arc<[u8]>> {
        self.resolve_read(fid, root, None)
    }

    pub(crate) fn read_attempt_with_packages(
        &self,
        fid: FileId,
        root: &Path,
        pkgs: (&PackageObservations, &crate::BundleCancellation),
    ) -> ReadAttempt<Arc<[u8]>> {
        self.resolve_read(fid, root, Some(pkgs))
    }

    fn resolve_read(
        &self,
        fid: FileId,
        root: &Path,
        pkg_obs: Option<(&PackageObservations, &crate::BundleCancellation)>,
    ) -> ReadAttempt<Arc<[u8]>> {
        if fid == *EMPTY_ID {
            return ReadAttempt::ok_provider(
                Arc::from(&b""[..]),
                ReadLocator::NonPersistent("<empty>".to_owned()),
                ReadOrigin::NonPersistent,
                Vec::new(),
            );
        }
        if fid == *STDIN_ID {
            return match read_stdin() {
                Ok(buf) => ReadAttempt::ok_provider(
                    buf.into(),
                    ReadLocator::NonPersistent("<stdin>".to_owned()),
                    ReadOrigin::NonPersistent,
                    Vec::new(),
                ),
                Err(err) => ReadAttempt::bare(Err(err)),
            };
        }

        let non_persistent = non_persistent_label(fid);

        if let Some(target) = self.provider.target(fid) {
            return match target {
                FileTarget::Bytes(buf) => ReadAttempt::ok_provider(
                    buf,
                    file_locator(fid, non_persistent, true),
                    ReadOrigin::Provider,
                    Vec::new(),
                ),
                FileTarget::Disk(path) => {
                    self.read_disk_target(fid, non_persistent, path, true, Vec::new(), None)
                }
                FileTarget::Missing => {
                    ReadAttempt::bare(Err(FileError::NotFound(reported_path(fid))))
                }
            };
        }

        if let typst::syntax::VirtualRoot::Package(spec) = fid.root() {
            if self
                .provider
                .owned_namespaces()
                .contains(&spec.namespace.as_str())
            {
                return ReadAttempt::bare(Err(FileError::Package(
                    typst::diag::PackageError::NotFound(spec.clone()),
                )));
            }
            if let Some(pkg) = self.local_packages.package(spec) {
                if fid.vpath().get_without_slash() == "typst.toml" {
                    return ReadAttempt::ok_provider(
                        pkg.manifest(),
                        file_locator(fid, non_persistent, true),
                        ReadOrigin::Provider,
                        Vec::new(),
                    );
                }
                let path = match fid.vpath().realize(pkg.root()) {
                    Ok(path) => path,
                    Err(_) => return ReadAttempt::err(FileError::AccessDenied, Vec::new()),
                };
                return self.read_disk_target(
                    fid,
                    non_persistent,
                    path,
                    true,
                    Vec::new(),
                    Some(pkg.boundary()),
                );
            }
            if self.local_packages.owns_namespace(spec.namespace.as_str()) {
                return ReadAttempt::bare(Err(FileError::Package(
                    typst::diag::PackageError::NotFound(spec.clone()),
                )));
            }
        }

        let (path, checks) = match self.resolve_path(root, fid, pkg_obs) {
            Ok(resolved) => resolved,
            Err((err, checks)) => return ReadAttempt::err(err, checks),
        };
        self.read_disk_target(fid, non_persistent, path, false, checks, None)
    }

    fn read_disk_target(
        &self,
        fid: FileId,
        non_persistent: Option<String>,
        path: PathBuf,
        provider_selected: bool,
        package_checks: Vec<PackageCheck>,
        pkg_boundary: Option<&crate::world::SourceBoundary>,
    ) -> ReadAttempt<Arc<[u8]>> {
        if provider_selected && !path.is_absolute() {
            return ReadAttempt::err(FileError::AccessDenied, package_checks);
        }
        let path = if path.is_absolute() {
            path
        } else {
            match std::path::absolute(path) {
                Ok(path) => path,
                Err(err) => {
                    return ReadAttempt::err(
                        FileError::from_io(err, &reported_path(fid)),
                        package_checks,
                    );
                }
            }
        };
        if let Some(boundary) = pkg_boundary
            && let Err(err) = check_pkg_boundary(boundary, &path, fid)
        {
            return ReadAttempt::err(err, package_checks);
        }
        if let Err(err) = self.boundary.check(&path) {
            return ReadAttempt::err(err, package_checks);
        }
        if let Some(buf) = self.overridden_bytes(&path) {
            return ReadAttempt::ok_provider(
                Arc::clone(buf),
                file_locator(fid, non_persistent, true),
                ReadOrigin::Provider,
                package_checks,
            );
        }
        let disk_read = match DiskReadPath::new(path) {
            Ok(path) => path,
            Err(err) => return ReadAttempt::err(err, package_checks),
        };
        match read_disk(disk_read.as_path(), &reported_path(fid)) {
            Ok(buf) => {
                let evidence =
                    ReadRecord::new(file_locator(fid, non_persistent, provider_selected), &buf);
                ReadAttempt {
                    result: Ok(buf.into()),
                    read: Some(FileRead::new(evidence, ReadOrigin::Disk(disk_read.clone()))),
                    disk_reads: vec![disk_read],
                    package_checks,
                }
            }
            Err(err) => ReadAttempt {
                disk_reads: vec![disk_read.with_error(&err)],
                result: Err(err),
                read: None,
                package_checks,
            },
        }
    }

    fn resolve_path(
        &self,
        root: &Path,
        fid: FileId,
        pkg_obs: Option<(&PackageObservations, &crate::BundleCancellation)>,
    ) -> Result<(PathBuf, Vec<PackageCheck>), (FileError, Vec<PackageCheck>)> {
        let (root, checks) = match fid.root() {
            typst::syntax::VirtualRoot::Project => (root.to_path_buf(), Vec::new()),
            typst::syntax::VirtualRoot::Package(spec) => {
                if let Some((obs, cancel)) = pkg_obs {
                    let pkg = obs.observe(spec, &self.packages, cancel);
                    match pkg.directory {
                        Ok(dir) => (dir, pkg.checks),
                        Err(err) => return Err((err, pkg.checks)),
                    }
                } else {
                    match self.packages.prepare(spec) {
                        Ok(prepared) => prepared.into_directory_and_checks(),
                        Err(failure) => return Err(failure.into_error_and_checks()),
                    }
                }
            }
        };

        match fid.vpath().realize(&root) {
            Ok(path) => Ok((path, checks)),
            Err(_) => Err((FileError::AccessDenied, checks)),
        }
    }
}

/// A read-only provider layered above another.
struct OverlayFiles {
    overlay: Arc<dyn FileProvider>,
    base: Arc<dyn FileProvider>,
}

impl FileProvider for OverlayFiles {
    fn target(&self, fid: FileId) -> Option<FileTarget> {
        self.overlay.target(fid).or_else(|| self.base.target(fid))
    }

    fn owned_namespaces(&self) -> &'static [&'static str] {
        self.base.owned_namespaces()
    }
}

fn check_pkg_boundary(
    boundary: &crate::world::SourceBoundary,
    path: &Path,
    fid: FileId,
) -> FileResult<()> {
    if boundary.refusal(path)?.is_some() {
        return Err(FileError::Other(Some(
            format!(
                "package source `{}` is outside its declared package root",
                reported_path(fid).display(),
            )
            .into(),
        )));
    }
    Ok(())
}

fn file_locator(fid: FileId, non_persistent: Option<String>, is_virtual: bool) -> ReadLocator {
    if let Some(label) = non_persistent {
        return ReadLocator::NonPersistent(label);
    }

    let path = vpath_rel(fid);
    match (fid.root(), is_virtual) {
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

fn vpath_rel(fid: FileId) -> PathBuf {
    PathBuf::from(fid.vpath().get_with_slash().trim_start_matches('/'))
}

/// The path a caller recognizes for one file ID, never a host path.
fn reported_path(fid: FileId) -> PathBuf {
    let path = vpath_rel(fid);
    match fid.root() {
        typst::syntax::VirtualRoot::Project => path,
        typst::syntax::VirtualRoot::Package(spec) => PathBuf::from(spec.to_string()).join(path),
    }
}

impl Default for FileStore {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use typst::syntax::FileId;

    #[test]
    fn package_verifier_refuses_read() {
        use crate::world::package::{PackageSpec, PackageVerificationError};
        use typst::diag::PackageError;

        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let root = data.join("local/demo/1.0.0");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("lib.typ"), "package source").unwrap();
        let locations = PackageLocations::from_absolute_roots(Some(data), None).unwrap();
        let fid = FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package("@local/demo:1.0.0".parse().unwrap()),
            typst::syntax::VirtualPath::new("lib.typ").unwrap(),
        ));
        let resolver = FileStore::from_package_locations(locations, PackageFetchMode::LocalOnly)
            .with_package_verifier(Arc::new(|_: &PackageSpec, _: &Path| {
                Err(PackageVerificationError::new(
                    "package `@local/demo:1.0.0` is not recorded in `tola.lock`",
                ))
            }));

        let attempt = resolver.read_attempt(fid, dir.path());

        assert!(matches!(
            attempt.result,
            Err(FileError::Package(PackageError::Other(Some(_))))
        ));
        assert!(attempt.read.is_none());
        assert!(
            attempt
                .package_checks
                .iter()
                .any(PackageCheck::was_selected)
        );
    }
}
