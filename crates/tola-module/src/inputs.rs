//! Read and record the directory every declared module reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use tola_input::{
    INTERNAL_DIR, InputLock, InputMode, InputScope, LockError, LockedModule, LockedRevision,
    Sha256Digest, TarballLocation, TreeHashError, layout, tree_sha256,
};
use tola_typst::BundleCancellation;

use crate::declaration::{ModuleDeclaration, ModuleSource, RemoteRevision, TarballSource};
use crate::identity::{ModuleAlias, ModuleId};
use crate::{BuiltinModule, checkout, remote};

/// Compute the vendored directory path for a site module or scoped child module (`parent/child`).
pub fn vendor_module_dir(vendor: &Path, id: &ModuleId) -> PathBuf {
    let mut path = vendor.join("modules");
    let mut segments = id.segments();
    if let Some(first) = segments.next() {
        path.push(first);
        for seg in segments {
            path.push("modules");
            path.push(seg);
        }
    }
    path
}

pub(crate) struct AcquireTarget<'a> {
    pub(crate) scoped_alias: &'a ModuleId,
    pub(crate) lock_key: &'a ModuleAlias,
    pub(crate) owner_root: &'a Path,
}

impl AcquireTarget<'_> {
    pub(crate) fn is_child(&self) -> bool {
        !self.scoped_alias.is_top_level()
    }
}

pub(crate) fn acquire_one(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    target: &AcquireTarget<'_>,
    decl: &ModuleDeclaration,
    lock_entries: Option<&mut BTreeMap<String, LockedModule>>,
    cancellation: &BundleCancellation,
) -> Result<PathBuf, ModuleRootError> {
    ModuleAcquisition {
        site_root,
        mode,
        vendor,
        cancellation,
    }
    .read_target(target, decl, lock_entries)
}
/// The directory every declared module reads, under the cancellation shared with its host.
pub fn module_roots_with_bundle(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    cancellation: &BundleCancellation,
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    let site_root = site_root
        .canonicalize()
        .map_err(|source| ModuleRootError::SiteRoot { source })?;
    let mut lock = InputLock::read(&site_root)?.unwrap_or_default();
    let original = lock.clone();
    let roots = record_module_roots_with_bundle(
        &site_root,
        mode,
        vendor,
        declarations,
        &mut lock,
        cancellation,
    )?;
    if lock != original && mode.may_write_lock() {
        ensure_active(cancellation)?;
        lock.commit(&site_root, &original)?;
    }
    Ok(roots)
}

/// Record the root and digest of every declared module into `lock`, fetching when allowed.
pub fn record_module_roots(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    lock: &mut InputLock,
    cancellation: &BundleCancellation,
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    record_module_roots_with_bundle(site_root, mode, vendor, declarations, lock, cancellation)
}

/// Record every declared module's root and digest under the cancellation shared with its host.
pub fn record_module_roots_with_bundle(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    lock: &mut InputLock,
    cancellation: &BundleCancellation,
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    ModuleAcquisition {
        site_root,
        mode,
        vendor,
        cancellation,
    }
    .record(declarations, &mut lock.modules, |_, _| {})
}

/// Forget the fact of every selected module, then read and record it again.
///
/// Calls `started` before reading each module.
///
/// Refreshing a module starts by dropping what the lock knew, so the next resolution asks the
/// repository for the declared selector instead of reusing the recorded commit.
pub fn refresh_module_roots_with_bundle(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    lock: &mut InputLock,
    cancellation: &BundleCancellation,
    started: impl FnMut(&str, &ModuleSource),
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    for alias in declarations.keys() {
        lock.modules.remove(alias);
    }
    ModuleAcquisition {
        site_root,
        mode,
        vendor,
        cancellation,
    }
    .record(declarations, &mut lock.modules, started)
}

/// The directory one declaration points at, without fetching or verifying it.
///
/// Use this for observation and watch paths. A build reads roots through [`module_roots_with_bundle`].
pub fn declaration_root(
    site_root: &Path,
    vendor: Option<&Path>,
    alias: &str,
    source: &ModuleSource,
) -> Result<PathBuf, ModuleRootError> {
    match source {
        ModuleSource::Local { path } => Ok(site_root.join(path)),
        ModuleSource::Builtin { builtin } => Ok(builtin_module(alias, builtin)?.root(site_root)),
        ModuleSource::Tarball { tarball } => {
            let id = ModuleId::parse(alias).map_err(|_| ModuleRootError::NotLocked {
                alias: alias.to_owned(),
            })?;
            let lock = InputLock::read(site_root)?.unwrap_or_default();
            let (digest, _) = match_tarball_lock(&lock, alias, tarball)?.ok_or_else(|| {
                ModuleRootError::NotLocked {
                    alias: alias.to_owned(),
                }
            })?;
            Ok(frozen_root(
                site_root,
                InputMode::new(InputScope::Online, false),
                vendor,
                &id,
            )?
            .unwrap_or_else(|| tarball_cache(site_root, digest)))
        }
        ModuleSource::Git { git } => {
            let id = ModuleId::parse(alias).map_err(|_| ModuleRootError::NotLocked {
                alias: alias.to_owned(),
            })?;
            let lock = InputLock::read(site_root)?.unwrap_or_default();
            let (commit, _) =
                match_git_lock(&lock, alias, &git.url, git.revision.as_ref(), &git.subdir)?
                    .ok_or_else(|| ModuleRootError::NotLocked {
                        alias: alias.to_owned(),
                    })?;
            select_root(
                site_root,
                InputMode::new(InputScope::Online, false),
                vendor,
                &id,
                &commit,
                &git.subdir,
            )
        }
    }
}

fn match_tarball_lock(
    lock: &InputLock,
    alias: &str,
    src: &TarballSource,
) -> Result<Option<(Sha256Digest, Sha256Digest)>, ModuleRootError> {
    match_tarball_lock_in(&lock.modules, alias, alias, src)
}

fn match_tarball_lock_in(
    entries: &BTreeMap<String, LockedModule>,
    scoped_alias: &str,
    lock_key: &str,
    src: &TarballSource,
) -> Result<Option<(Sha256Digest, Sha256Digest)>, ModuleRootError> {
    match entries.get(lock_key) {
        Some(LockedModule::Tarball {
            tarball,
            tarball_sha256,
            tree_sha256,
            ..
        }) if tarball == &src.location && src.sha256.is_none_or(|exp| exp == *tarball_sha256) => {
            Ok(Some((*tarball_sha256, *tree_sha256)))
        }
        Some(_) => Err(ModuleRootError::DeclarationChanged {
            alias: scoped_alias.to_owned(),
        }),
        None => Ok(None),
    }
}

fn match_git_lock(
    lock: &InputLock,
    alias: &str,
    url: &str,
    rev: Option<&RemoteRevision>,
    subdir: &Path,
) -> Result<Option<(String, Sha256Digest)>, ModuleRootError> {
    match_git_lock_in(&lock.modules, alias, alias, url, rev, subdir)
}

fn match_git_lock_in(
    entries: &BTreeMap<String, LockedModule>,
    scoped_alias: &str,
    lock_key: &str,
    url: &str,
    rev: Option<&RemoteRevision>,
    subdir: &Path,
) -> Result<Option<(String, Sha256Digest)>, ModuleRootError> {
    let sel = locked_revision(rev);
    match entries.get(lock_key) {
        Some(LockedModule::Git {
            git,
            revision,
            subdir: rec_subdir,
            commit,
            tree_sha256,
            ..
        }) if git == url && revision == &sel && rec_subdir == subdir => {
            Ok(Some((commit.clone(), *tree_sha256)))
        }
        Some(_) => Err(ModuleRootError::DeclarationChanged {
            alias: scoped_alias.to_owned(),
        }),
        None => Ok(None),
    }
}

fn ensure_inside_site(site: &Path, alias: &str, path: &Path) -> Result<(), ModuleRootError> {
    let canonical = path
        .canonicalize()
        .map_err(|source| ModuleRootError::Root {
            alias: alias.to_owned(),
            path: path.to_path_buf(),
            source,
        })?;
    if !canonical.starts_with(site) {
        return Err(ModuleRootError::OutsideSite { path: canonical });
    }
    Ok(())
}

fn vendor_missing(vendor: Option<&Path>, id: &ModuleId) -> ModuleRootError {
    ModuleRootError::VendorMissing {
        alias: id.as_str().to_owned(),
        path: vendor_module_dir(vendor.unwrap_or(Path::new("vendor")), id),
    }
}

fn network_denied(mode: InputMode, alias: &str) -> ModuleRootError {
    ModuleRootError::NetworkDenied {
        alias: alias.to_owned(),
        scope: mode.scope().as_str(),
    }
}

/// The site, mode, frozen copy, and cancellation one attempt reads module roots under.
struct ModuleAcquisition<'a> {
    site_root: &'a Path,
    mode: InputMode,
    vendor: Option<&'a Path>,
    cancellation: &'a BundleCancellation,
}

impl ModuleAcquisition<'_> {
    fn record(
        &self,
        declarations: &BTreeMap<String, ModuleDeclaration>,
        lock_entries: &mut BTreeMap<String, LockedModule>,
        mut started: impl FnMut(&str, &ModuleSource),
    ) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
        let mut roots = BTreeMap::new();
        for (alias, decl) in declarations {
            ensure_active(self.cancellation)?;
            started(alias, &decl.source);
            let lock_key = ModuleAlias::parse(alias).map_err(|_| ModuleRootError::NotLocked {
                alias: alias.clone(),
            })?;
            let scoped_alias = ModuleId::top_level(lock_key.clone());
            let target = AcquireTarget {
                scoped_alias: &scoped_alias,
                lock_key: &lock_key,
                owner_root: self.site_root,
            };
            let root = self.read_target(&target, decl, Some(lock_entries))?;
            roots.insert(alias.clone(), root);
        }
        Ok(roots)
    }

    fn read_target(
        &self,
        target: &AcquireTarget<'_>,
        decl: &ModuleDeclaration,
        lock_entries: Option<&mut BTreeMap<String, LockedModule>>,
    ) -> Result<PathBuf, ModuleRootError> {
        let alias = target.scoped_alias.as_str();
        match &decl.source {
            ModuleSource::Local { path } => self.local(target, path, lock_entries),
            ModuleSource::Git { git } => {
                let lock_entries = lock_entries.ok_or_else(|| ModuleRootError::UnlockedParent {
                    alias: alias.to_owned(),
                })?;
                self.remote(
                    target.scoped_alias,
                    target.lock_key.as_str(),
                    &git.url,
                    git.revision.as_ref(),
                    &git.subdir,
                    lock_entries,
                )
            }
            ModuleSource::Tarball { tarball } => {
                let lock_entries = lock_entries.ok_or_else(|| ModuleRootError::UnlockedParent {
                    alias: alias.to_owned(),
                })?;
                self.tarball(target, tarball, lock_entries)
            }
            ModuleSource::Builtin { builtin } => {
                let pkg = builtin_module(alias, builtin)?;
                let root = materialize_builtin(self.site_root, alias, pkg, self.cancellation)?;
                if let Some(entries) = lock_entries {
                    entries.remove(target.lock_key.as_str());
                }
                Ok(root)
            }
        }
    }

    fn local(
        &self,
        target: &AcquireTarget<'_>,
        path: &Path,
        lock_entries: Option<&mut BTreeMap<String, LockedModule>>,
    ) -> Result<PathBuf, ModuleRootError> {
        let root = self.local_root(target, path)?;
        if let Some(entries) = lock_entries {
            self.record_local(target, path, &root, entries)?;
        }
        Ok(root)
    }

    fn local_root(
        &self,
        target: &AcquireTarget<'_>,
        path: &Path,
    ) -> Result<PathBuf, ModuleRootError> {
        let alias = target.scoped_alias.as_str();
        let declared = target.owner_root.join(path);
        if !declared.is_dir() {
            return Err(ModuleRootError::NotDirectory {
                alias: alias.to_owned(),
                path: declared,
            });
        }
        if target.is_child() {
            let owner_canonical =
                target
                    .owner_root
                    .canonicalize()
                    .map_err(|source| ModuleRootError::Root {
                        alias: alias.to_owned(),
                        path: target.owner_root.to_path_buf(),
                        source,
                    })?;
            let child_canonical =
                declared
                    .canonicalize()
                    .map_err(|source| ModuleRootError::Root {
                        alias: alias.to_owned(),
                        path: declared.clone(),
                        source,
                    })?;
            if !child_canonical.starts_with(&owner_canonical) {
                return Err(ModuleRootError::ChildOutsideParent {
                    alias: alias.to_owned(),
                    path: path.to_path_buf(),
                });
            }
            return Ok(child_canonical);
        }
        if self.mode.scope() == InputScope::Pure {
            ensure_inside_site(self.site_root, alias, &declared)?;
        }
        Ok(declared)
    }

    fn record_local(
        &self,
        target: &AcquireTarget<'_>,
        path: &Path,
        root: &Path,
        lock_entries: &mut BTreeMap<String, LockedModule>,
    ) -> Result<(), ModuleRootError> {
        let tree_sha256 = tree_digest(target.scoped_alias.as_str(), root)?;
        let recorded = path
            .strip_prefix(target.owner_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf());
        let existing_children = lock_entries
            .get(target.lock_key.as_str())
            .map(|entry| entry.children().clone())
            .unwrap_or_default();
        lock_entries.insert(
            target.lock_key.as_str().to_owned(),
            LockedModule::Local {
                path: recorded,
                tree_sha256,
                modules: existing_children,
            },
        );
        Ok(())
    }

    fn remote(
        &self,
        scoped_id: &ModuleId,
        lock_key: &str,
        git: &str,
        rev: Option<&RemoteRevision>,
        subdir: &Path,
        lock_entries: &mut BTreeMap<String, LockedModule>,
    ) -> Result<PathBuf, ModuleRootError> {
        let scoped_alias = scoped_id.as_str();
        let recorded = match_git_lock_in(lock_entries, scoped_alias, lock_key, git, rev, subdir)?;
        let vendor = recorded.as_ref().and(self.vendor);
        let (commit, digest) = match recorded {
            Some(entry) => entry,
            None if self.mode.may_write_lock() => {
                let (commit, digest) = self.fetch(scoped_alias, git, rev, subdir)?;
                lock_entries.insert(
                    lock_key.to_owned(),
                    LockedModule::Git {
                        git: git.to_owned(),
                        revision: locked_revision(rev),
                        subdir: subdir.to_path_buf(),
                        commit: commit.clone(),
                        tree_sha256: digest,
                        modules: BTreeMap::new(),
                    },
                );
                (commit, digest)
            }
            None => {
                return Err(ModuleRootError::NotLocked {
                    alias: scoped_alias.to_owned(),
                });
            }
        };
        let root = select_root(
            self.site_root,
            self.mode,
            vendor,
            scoped_id,
            &commit,
            subdir,
        )?;
        if !root.exists() {
            let pinned = RemoteRevision::Rev(commit);
            let (_, found) = self.fetch(scoped_alias, git, Some(&pinned), subdir)?;
            if found != digest {
                return Err(ModuleRootError::DigestMismatch {
                    alias: scoped_alias.to_owned(),
                    path: root,
                    expected: digest,
                    found,
                });
            }
        }
        verify_tree(scoped_alias, &root, digest)?;
        Ok(root)
    }

    fn tarball(
        &self,
        target: &AcquireTarget<'_>,
        src: &TarballSource,
        lock_entries: &mut BTreeMap<String, LockedModule>,
    ) -> Result<PathBuf, ModuleRootError> {
        ensure_active(self.cancellation)?;
        let scoped_alias = target.scoped_alias.as_str();
        let recorded =
            match_tarball_lock_in(lock_entries, scoped_alias, target.lock_key.as_str(), src)?;
        if recorded.is_none() && !self.mode.may_write_lock() {
            return Err(ModuleRootError::NotLocked {
                alias: scoped_alias.to_owned(),
            });
        }
        if let Some((digest, tree)) = recorded {
            if let Some(root) =
                frozen_root(self.site_root, self.mode, self.vendor, target.scoped_alias)?
            {
                verify_tree(scoped_alias, &root, tree)?;
                return Ok(root);
            }
            if self.mode.may_read_cache() {
                let root = tarball_cache(self.site_root, digest);
                if root.exists() {
                    verify_tree(scoped_alias, &root, tree)?;
                    return Ok(root);
                }
            } else if matches!(src.location, TarballLocation::Url { .. }) {
                return Err(vendor_missing(self.vendor, target.scoped_alias));
            }
        }
        let result = self.materialize_tarball(target, src, recorded);
        ensure_active(self.cancellation)?;
        let (root, digest, tree) = result?;
        if recorded.is_none() {
            lock_entries.insert(
                target.lock_key.as_str().to_owned(),
                LockedModule::Tarball {
                    tarball: src.location.clone(),
                    tarball_sha256: digest,
                    tree_sha256: tree,
                    modules: BTreeMap::new(),
                },
            );
        }
        Ok(root)
    }

    fn materialize_tarball(
        &self,
        target: &AcquireTarget<'_>,
        src: &TarballSource,
        recorded: Option<(Sha256Digest, Sha256Digest)>,
    ) -> Result<(PathBuf, Sha256Digest, Sha256Digest), ModuleRootError> {
        let alias = target.scoped_alias.as_str();
        let failure = |stage, cause: anyhow::Error| ModuleRootError::Tarball {
            alias: alias.to_owned(),
            location: src.location.clone(),
            stage,
            source: cause,
        };
        let download = match &src.location {
            TarballLocation::Url { url } => {
                if !self.mode.may_use_network() {
                    return Err(network_denied(self.mode, alias));
                }
                crate::archive::from_url(url, self.cancellation)
            }
            TarballLocation::File { path } => {
                let path = target.owner_root.join(path);
                if target.is_child() {
                    let owner_canonical = target.owner_root.canonicalize().map_err(|source| {
                        ModuleRootError::Root {
                            alias: alias.to_owned(),
                            path: target.owner_root.to_path_buf(),
                            source,
                        }
                    })?;
                    let archive_canonical =
                        path.canonicalize()
                            .map_err(|source| ModuleRootError::Root {
                                alias: alias.to_owned(),
                                path: path.clone(),
                                source,
                            })?;
                    if !archive_canonical.starts_with(&owner_canonical) {
                        return Err(ModuleRootError::ChildOutsideParent {
                            alias: alias.to_owned(),
                            path: path.to_path_buf(),
                        });
                    }
                } else if self.mode.scope() == InputScope::Pure {
                    if tola_input::FilesystemSourceIdentity::from_path(&path).intersects(
                        &tola_input::FilesystemSourceIdentity::from_path(
                            &self.site_root.join(INTERNAL_DIR),
                        ),
                    ) {
                        return Err(ModuleRootError::InternalArchive {
                            alias: alias.to_owned(),
                        });
                    }
                    ensure_inside_site(self.site_root, alias, &path)?;
                }
                crate::archive::from_file(&path, self.cancellation)
            }
        };
        ensure_active(self.cancellation)?;
        let mut download = download.map_err(|err| failure(ArchiveStage::Read, err))?;
        let expected = src.sha256.or(recorded.map(|(digest, _)| digest));
        if let Some(expected) = expected
            && expected != download.digest
        {
            return Err(ModuleRootError::TarballDigestMismatch {
                alias: alias.to_owned(),
                declared: src.sha256.is_some(),
                expected,
                found: download.digest,
            });
        }
        let dst = tarball_cache(self.site_root, download.digest);
        let parent = dst.parent().expect("module cache has a parent");
        std::fs::create_dir_all(parent)
            .context("could not prepare the module cache")
            .map_err(|err| failure(ArchiveStage::Cache, err))?;
        let stage = tola_fs::DirectoryReplacement::new(&dst)
            .context("could not stage the module directory")
            .map_err(|err| failure(ArchiveStage::Cache, err))?;
        let root = stage.staged();
        crate::archive::extract(download.file.as_file_mut(), &root, self.cancellation)
            .map_err(|err| failure(ArchiveStage::Extract, err))?;
        let manifest = crate::manifest::read_one(
            &root,
            alias,
            root.clone(),
            InputMode::new(InputScope::Online, false),
            &[],
        )
        .map_err(|err| failure(ArchiveStage::Manifest, err.into()))?;
        crate::check::check_loaded(&manifest)
            .map_err(|err| failure(ArchiveStage::Manifest, err))?;
        let tree = tree_digest(alias, &root)?;
        if let Some((_, expected)) = recorded
            && expected != tree
        {
            return Err(ModuleRootError::DigestMismatch {
                alias: alias.to_owned(),
                path: dst,
                expected,
                found: tree,
            });
        }
        ensure_active(self.cancellation)?;
        stage
            .publish()
            .context("could not publish the module cache")
            .map_err(|err| failure(ArchiveStage::Cache, err))?;
        Ok((dst, download.digest, tree))
    }

    fn fetch(
        &self,
        alias: &str,
        git: &str,
        rev: Option<&RemoteRevision>,
        subdir: &Path,
    ) -> Result<(String, Sha256Digest), ModuleRootError> {
        if !self.mode.may_use_network() {
            return Err(network_denied(self.mode, alias));
        }
        let staging = stage(self.site_root)?;
        let commit = acquire(alias, git, rev, &staging, self.cancellation)?;
        if let Some(RemoteRevision::Rev(expected)) = rev
            && expected != &commit
        {
            return Err(ModuleRootError::CommitMismatch {
                alias: alias.to_owned(),
                expected: expected.clone(),
                found: commit,
            });
        }
        let root = select_subdir(alias, &staging.source(), subdir, &commit)?;
        let digest = tree_digest(alias, &root)?;
        store_checkout(
            alias,
            &root,
            &cache_root(self.site_root, &commit, subdir),
            digest,
        )?;
        Ok((commit, digest))
    }
}

fn builtin_module(alias: &str, name: &str) -> Result<&'static BuiltinModule, ModuleRootError> {
    BuiltinModule::named(name).ok_or_else(|| ModuleRootError::UnknownBuiltin {
        alias: alias.to_owned(),
        name: name.to_owned(),
    })
}

fn materialize_builtin(
    site: &Path,
    alias: &str,
    builtin: &BuiltinModule,
    cancellation: &BundleCancellation,
) -> Result<PathBuf, ModuleRootError> {
    let result = (|| -> Result<PathBuf> {
        let destination = builtin.root(site);
        let canonical_site = tola_input::normalize_existing_prefix(site);
        anyhow::ensure!(
            tola_input::normalize_existing_prefix(&destination).starts_with(&canonical_site),
            "the module directory points outside the site"
        );
        let staging = stage(site)?;
        let source = staging.source();
        for (file, bytes) in builtin.files {
            let path = source.join(file);
            std::fs::create_dir_all(path.parent().expect("embedded file has a parent"))?;
            std::fs::write(path, bytes)?;
        }
        ensure_active(cancellation)?;
        let expected = tree_digest(alias, &source)?;
        store_checkout(alias, &source, &destination, expected)?;
        Ok(destination)
    })();
    result.map_err(|source| ModuleRootError::Builtin {
        alias: alias.to_owned(),
        name: builtin.name,
        source,
    })
}

/// One fetch attempt's staging area, removed unless its source tree moves into the cache.
struct Staging {
    /// The temporary directory below `.tola` that holds the repository and its checkout.
    directory: tempfile::TempDir,
}

impl Staging {
    /// Where the bare repository is fetched.
    fn git_dir(&self) -> PathBuf {
        self.directory.path().join("objects.git")
    }

    /// Where the fetched tree is materialized.
    fn source(&self) -> PathBuf {
        self.directory.path().join("source")
    }
}

/// Prepare the cache directory and the temporary workspace one fetch writes through.
fn stage(site_root: &Path) -> Result<Staging, ModuleRootError> {
    let internal = site_root.join(INTERNAL_DIR);
    std::fs::create_dir_all(&internal).map_err(|source| ModuleRootError::Io {
        action: "create",
        path: internal.clone(),
        source,
    })?;
    let directory = tempfile::tempdir_in(&internal).map_err(|source| ModuleRootError::Io {
        action: "stage",
        path: internal,
        source,
    })?;
    Ok(Staging { directory })
}

/// Fetch the declared revision and materialize its tree into the staging area.
fn acquire(
    alias: &str,
    git: &str,
    revision: Option<&RemoteRevision>,
    staging: &Staging,
    cancellation: &BundleCancellation,
) -> Result<String, ModuleRootError> {
    let git_dir = staging.git_dir();
    let request = remote::FetchRequest {
        git,
        revision,
        git_dir: &git_dir,
        options: remote::FetchOptions::default(),
    };
    let fetched = match remote::fetch(request, cancellation) {
        Ok(fetched) => fetched,
        Err(source) => {
            ensure_active(cancellation)?;
            return Err(ModuleRootError::Fetch {
                alias: alias.to_owned(),
                revision: revision.cloned(),
                source,
            });
        }
    };
    ensure_active(cancellation)?;
    let root = staging.source();
    if let Err(source) =
        checkout::materialize(&fetched.repository, fetched.tree, &root, cancellation)
    {
        ensure_active(cancellation)?;
        return Err(ModuleRootError::Extract {
            alias: alias.to_owned(),
            source,
        });
    }
    ensure_active(cancellation)?;
    Ok(fetched.commit)
}

/// Reject resolution once the build's cancellation flag is set.
fn ensure_active(cancellation: &BundleCancellation) -> Result<(), ModuleRootError> {
    if cancellation.is_cancelled() {
        Err(ModuleRootError::Cancelled(
            tola_typst::CompileError::Cancelled,
        ))
    } else {
        Ok(())
    }
}

/// The declared subdirectory of one snapshot, which must stay inside it and name a directory.
fn select_subdir(
    alias: &str,
    snapshot: &Path,
    subdir: &Path,
    commit: &str,
) -> Result<PathBuf, ModuleRootError> {
    let failure = |source| ModuleRootError::Subdir {
        alias: alias.to_owned(),
        subdir: subdir.to_owned(),
        commit: commit.to_owned(),
        source,
    };
    let declared = snapshot.join(subdir);
    let root = declared
        .canonicalize()
        .map_err(|source| failure(Some(source)))?;
    let snapshot = snapshot
        .canonicalize()
        .map_err(|source| failure(Some(source)))?;
    if !root.starts_with(&snapshot) || !root.is_dir() {
        return Err(failure(None));
    }
    Ok(root)
}

/// The digest of one module directory.
fn tree_digest(alias: &str, root: &Path) -> Result<Sha256Digest, ModuleRootError> {
    tree_sha256(root).map_err(|source| ModuleRootError::Tree {
        alias: alias.to_owned(),
        path: root.to_path_buf(),
        source,
    })
}

fn verify_tree(alias: &str, root: &Path, expected: Sha256Digest) -> Result<(), ModuleRootError> {
    let found = tree_digest(alias, root)?;
    if found != expected {
        return Err(ModuleRootError::DigestMismatch {
            alias: alias.to_owned(),
            path: root.to_owned(),
            expected,
            found,
        });
    }
    Ok(())
}

fn tarball_cache(site_root: &Path, digest: Sha256Digest) -> PathBuf {
    layout::modules(site_root)
        .join("tarballs")
        .join(digest.to_string())
}

/// Keep only the selected module directory, shared by aliases of this revision and subdirectory.
fn store_checkout(
    alias: &str,
    root: &Path,
    destination: &Path,
    digest: Sha256Digest,
) -> Result<(), ModuleRootError> {
    let parent = destination.parent().expect("module cache has a parent");
    std::fs::create_dir_all(parent).map_err(|source| ModuleRootError::Io {
        action: "create the module cache",
        path: parent.to_owned(),
        source,
    })?;
    if destination.exists() {
        return verify_tree(alias, destination, digest);
    }
    match std::fs::rename(root, destination) {
        Ok(()) => Ok(()),
        // Another acquisition may have published the same commit while this one fetched it.
        Err(_) if destination.is_dir() => verify_tree(alias, destination, digest),
        Err(source) => Err(ModuleRootError::Io {
            action: "store",
            path: destination.to_owned(),
            source,
        }),
    }
}

/// Commit pins Git file modes as well as bytes; the subdirectory key separates module roots.
fn cache_root(site_root: &Path, commit: &str, subdir: &Path) -> PathBuf {
    let key = Sha256Digest::of_bytes(tola_diagnostic::render_relative(subdir).as_bytes());
    layout::modules(site_root)
        .join(commit)
        .join(key.to_string())
}

/// Prefer a frozen module directory; pure builds never read the disposable cache.
fn select_root(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    id: &ModuleId,
    commit: &str,
    subdir: &Path,
) -> Result<PathBuf, ModuleRootError> {
    if let Some(frozen) = frozen_root(site_root, mode, vendor, id)? {
        return Ok(frozen);
    }
    if mode.scope() == InputScope::Pure {
        return Err(vendor_missing(vendor, id));
    }
    Ok(cache_root(site_root, commit, subdir))
}

fn frozen_root(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    id: &ModuleId,
) -> Result<Option<PathBuf>, ModuleRootError> {
    let Some(root) = vendor
        .map(|path| vendor_module_dir(path, id))
        .filter(|path| path.is_dir())
    else {
        return Ok(None);
    };
    if mode.scope() == InputScope::Pure {
        ensure_inside_site(site_root, id.as_str(), &root)?;
    }
    Ok(Some(root))
}

/// The lock's spelling of one declared selector.
fn locked_revision(revision: Option<&RemoteRevision>) -> Option<LockedRevision> {
    revision.map(|revision| match revision {
        RemoteRevision::Tag(tag) => LockedRevision::Tag(tag.clone()),
        RemoteRevision::Branch(branch) => LockedRevision::Branch(branch.clone()),
        RemoteRevision::Rev(rev) => LockedRevision::Rev(rev.clone()),
    })
}

/// The archive operation that failed.
#[derive(Debug, Clone, Copy)]
pub enum ArchiveStage {
    Read,
    Cache,
    Extract,
    Manifest,
}

/// Why one declared module has no readable directory.
#[derive(Debug, thiserror::Error)]
pub enum ModuleRootError {
    #[error(
        "`modules.{alias}.builtin` names `{name}`, which is not shipped with this Tola executable"
    )]
    UnknownBuiltin { alias: String, name: String },
    #[error(
        "Tola could not prepare built-in module `{name}` for `{alias}`; check the site's `.tola/modules/builtin` directory"
    )]
    Builtin {
        alias: String,
        name: &'static str,
        #[source]
        source: anyhow::Error,
    },
    #[error(
        "module `{alias}` cannot read its archive from `.tola` under `--pure`; keep the archive in the site's source tree"
    )]
    InternalArchive { alias: String },
    #[error("Tola could not obtain the tarball for module `{alias}`: {source}")]
    Tarball {
        alias: String,
        location: TarballLocation,
        stage: ArchiveStage,
        #[source]
        source: anyhow::Error,
    },
    #[error(
        "the tarball for module `{alias}` does not match its checksum: expected {expected}, found {found}"
    )]
    TarballDigestMismatch {
        alias: String,
        declared: bool,
        expected: Sha256Digest,
        found: Sha256Digest,
    },
    #[error(
        "module `{alias}` needs a download, which `--{scope}` does not allow; fetch it online or provide its vendor copy"
    )]
    NetworkDenied { alias: String, scope: &'static str },
    #[error("module `{alias}` returned commit `{found}` instead of the requested `{expected}`")]
    CommitMismatch {
        alias: String,
        expected: String,
        found: String,
    },
    #[error(transparent)]
    Cancelled(#[from] tola_typst::CompileError),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error("Tola could not read the site root")]
    SiteRoot {
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not read `{path}` for module `{alias}`")]
    Root {
        alias: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` is not a directory that can provide module `{alias}`")]
    NotDirectory { alias: String, path: PathBuf },
    #[error("`{path}` is outside the site; `--pure` reads only site sources")]
    OutsideSite { path: PathBuf },
    #[error("Tola could not digest `{path}` for module `{alias}`")]
    Tree {
        alias: String,
        path: PathBuf,
        #[source]
        source: TreeHashError,
    },
    #[error("module `{alias}` is not locked; run `tola update {alias}`")]
    NotLocked { alias: String },
    #[error(
        "the declaration of module `{alias}` no longer matches `tola.lock`; run `tola update {alias}`"
    )]
    DeclarationChanged { alias: String },
    #[error("module `{alias}` has no frozen copy at `{path}`; run `tola vendor`")]
    VendorMissing { alias: String, path: PathBuf },
    #[error(
        "module `{alias}` at `{path}` does not match `tola.lock`: expected {expected}, found {found}"
    )]
    DigestMismatch {
        alias: String,
        path: PathBuf,
        expected: Sha256Digest,
        found: Sha256Digest,
    },
    #[error("module `{alias}` cannot read `{subdir}` as a directory in commit `{commit}`")]
    Subdir {
        alias: String,
        subdir: PathBuf,
        commit: String,
        #[source]
        source: Option<std::io::Error>,
    },
    #[error("Tola could not fetch module `{alias}`: {source}")]
    Fetch {
        alias: String,
        revision: Option<RemoteRevision>,
        #[source]
        source: crate::remote::FetchError,
    },
    #[error("Tola could not extract module `{alias}` from its revision: {source}")]
    Extract {
        alias: String,
        #[source]
        source: crate::checkout::CheckoutError,
    },
    #[error(
        "module `{alias}` requires a parent with a `tola.lock` entry to lock its remote source"
    )]
    UnlockedParent { alias: String },
    #[error("module `{alias}` points outside its parent module (`{path}`)")]
    ChildOutsideParent { alias: String, path: PathBuf },
    #[error("module declarations form a cycle: {}", chain.join(" -> "))]
    Cycle { chain: Vec<String> },
    #[error("Tola could not {action} `{path}`")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_package(path: &Path, library: &[u8]) {
        let gzip = flate2::write::GzEncoder::new(
            fs::File::create(path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut archive = tar::Builder::new(gzip);
        for (name, bytes) in [
            (
                "module/tola-module.toml",
                &b"[typst]\nentry = 'lib.typ'\n"[..],
            ),
            ("module/lib.typ", library),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            archive.append_data(&mut header, name, bytes).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn locked_tarball_cache_and_vendor() {
        let site = tempfile::tempdir().unwrap();
        let pkg = site.path().join("theme.tar.gz");
        write_package(&pkg, b"#let value = 1");
        let decls = BTreeMap::from([(
            "theme".into(),
            ModuleDeclaration {
                source: ModuleSource::Tarball {
                    tarball: TarballSource {
                        location: TarballLocation::File {
                            path: "theme.tar.gz".into(),
                        },
                        sha256: None,
                    },
                },
                ..Default::default()
            },
        )]);
        let cancel = BundleCancellation::default();
        let offline = InputMode::new(InputScope::Offline, false);
        let locked = InputMode::new(InputScope::Online, true);
        let pure = InputMode::new(InputScope::Pure, false);
        let roots = module_roots_with_bundle(site.path(), offline, None, &decls, &cancel).unwrap();
        let orig = InputLock::read(site.path()).unwrap().unwrap();
        fs::write(roots["theme"].join("lib.typ"), "tampered cache").unwrap();
        assert!(matches!(
            module_roots_with_bundle(
                site.path(),
                InputMode::new(InputScope::Online, false),
                None,
                &decls,
                &cancel
            ),
            Err(ModuleRootError::DigestMismatch { .. })
        ));
        module_roots_with_bundle(site.path(), pure, None, &decls, &cancel).unwrap();
        fs::remove_dir_all(&roots["theme"]).unwrap();
        module_roots_with_bundle(site.path(), locked, None, &decls, &cancel).unwrap();
        assert_eq!(InputLock::read(site.path()).unwrap().unwrap(), orig);
        write_package(&pkg, b"#let value = 2");
        fs::remove_dir_all(&roots["theme"]).unwrap();
        assert!(matches!(
            module_roots_with_bundle(site.path(), locked, None, &decls, &cancel),
            Err(ModuleRootError::TarballDigestMismatch { .. })
        ));
        assert_eq!(InputLock::read(site.path()).unwrap().unwrap(), orig);
        let mut updated = orig.clone();
        let roots = refresh_module_roots_with_bundle(
            site.path(),
            offline,
            None,
            &decls,
            &mut updated,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert_ne!(updated, orig);
        updated.commit(site.path(), &orig).unwrap();
        let vendor = site.path().join("vendor");
        let frozen = vendor.join("modules/theme");
        fs::create_dir_all(&frozen).unwrap();
        for entry in fs::read_dir(&roots["theme"]).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), frozen.join(entry.file_name())).unwrap();
        }
        fs::remove_file(pkg).unwrap();
        fs::remove_dir_all(&roots["theme"]).unwrap();
        let roots =
            module_roots_with_bundle(site.path(), pure, Some(&vendor), &decls, &cancel).unwrap();
        assert_eq!(roots["theme"], frozen);
    }
}
