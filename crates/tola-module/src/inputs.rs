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
use crate::{checkout, remote};

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
        lock,
        cancellation,
    }
    .record(declarations)
}

/// Forget the fact of every selected module, then read and record it again.
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
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    for alias in declarations.keys() {
        lock.modules.remove(alias);
    }
    record_module_roots(site_root, mode, vendor, declarations, lock, cancellation)
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
        ModuleSource::Tarball { tarball } => {
            let lock = InputLock::read(site_root)?.unwrap_or_default();
            match lock.modules.get(alias) {
                Some(LockedModule::Tarball {
                    tarball: recorded,
                    tarball_sha256,
                    ..
                }) if recorded == &tarball.location
                    && tarball
                        .sha256
                        .is_none_or(|expected| expected == *tarball_sha256) =>
                {
                    Ok(frozen_root(
                        site_root,
                        InputMode::new(InputScope::Online, false),
                        vendor,
                        alias,
                    )?
                    .unwrap_or_else(|| tarball_cache(site_root, *tarball_sha256)))
                }
                _ => Err(ModuleRootError::NotLocked {
                    alias: alias.to_owned(),
                }),
            }
        }
        ModuleSource::Git { git } => {
            let lock = InputLock::read(site_root)?.unwrap_or_default();
            let selector = locked_revision(git.revision.as_ref());
            match lock.modules.get(alias) {
                Some(LockedModule::Git {
                    git: recorded_git,
                    revision: recorded_revision,
                    subdir: recorded_subdir,
                    commit,
                    ..
                }) if recorded_git == &git.url
                    && recorded_revision == &selector
                    && recorded_subdir == &git.subdir =>
                {
                    select_root(
                        site_root,
                        InputMode::new(InputScope::Online, false),
                        vendor,
                        alias,
                        commit,
                        &git.subdir,
                    )
                }
                _ => Err(ModuleRootError::NotLocked {
                    alias: alias.to_owned(),
                }),
            }
        }
    }
}

/// The site, mode, frozen copy, lock, and cancellation one attempt reads module roots under.
struct ModuleAcquisition<'a> {
    site_root: &'a Path,
    mode: InputMode,
    vendor: Option<&'a Path>,
    lock: &'a mut InputLock,
    cancellation: &'a BundleCancellation,
}

impl ModuleAcquisition<'_> {
    /// Record the root and digest of every declaration into the lock.
    fn record(
        &mut self,
        declarations: &BTreeMap<String, ModuleDeclaration>,
    ) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
        let mut roots = BTreeMap::new();
        for (alias, declaration) in declarations {
            ensure_active(self.cancellation)?;
            let root = self.read(alias, declaration)?;
            roots.insert(alias.clone(), root);
        }
        Ok(roots)
    }

    /// The directory one declaration reads, and the fact the lock records for it.
    fn read(
        &mut self,
        alias: &str,
        declaration: &ModuleDeclaration,
    ) -> Result<PathBuf, ModuleRootError> {
        match &declaration.source {
            ModuleSource::Local { path } => self.local(alias, path),
            ModuleSource::Git { git } => {
                self.remote(alias, &git.url, git.revision.as_ref(), &git.subdir)
            }
            ModuleSource::Tarball { tarball } => self.tarball(alias, tarball),
        }
    }

    /// A local module's directory, recorded as the bytes a build reads.
    fn local(&mut self, alias: &str, path: &Path) -> Result<PathBuf, ModuleRootError> {
        let root = self.local_root(alias, path)?;
        self.record_local(alias, path, &root)?;
        Ok(root)
    }

    /// A local module's directory, which must stay inside the site.
    fn local_root(&self, alias: &str, path: &Path) -> Result<PathBuf, ModuleRootError> {
        let declared = self.site_root.join(path);
        if !declared.is_dir() {
            return Err(ModuleRootError::NotDirectory {
                alias: alias.to_owned(),
                path: declared,
            });
        }
        if self.mode.scope() == InputScope::Pure {
            let canonical = declared
                .canonicalize()
                .map_err(|source| ModuleRootError::Root {
                    alias: alias.to_owned(),
                    path: declared.clone(),
                    source,
                })?;
            if !canonical.starts_with(self.site_root) {
                return Err(ModuleRootError::OutsideSite { path: canonical });
            }
        }
        Ok(declared)
    }

    /// Record what a local module read, which only proves the bytes a build saw.
    fn record_local(
        &mut self,
        alias: &str,
        path: &Path,
        root: &Path,
    ) -> Result<(), ModuleRootError> {
        let tree_sha256 = tree_digest(alias, root)?;
        let recorded = path
            .strip_prefix(self.site_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf());
        let entry = LockedModule::Local {
            path: recorded,
            tree_sha256,
        };
        self.lock.modules.insert(alias.to_owned(), entry);
        Ok(())
    }

    /// A remote module's directory, fetched when this attempt may write and the lock has no fact.
    fn remote(
        &mut self,
        alias: &str,
        git: &str,
        revision: Option<&RemoteRevision>,
        subdir: &Path,
    ) -> Result<PathBuf, ModuleRootError> {
        let selector = locked_revision(revision);
        let recorded = match self.lock.modules.get(alias) {
            Some(LockedModule::Git {
                git: recorded_git,
                revision: recorded_revision,
                subdir: recorded_subdir,
                commit,
                tree_sha256,
            }) if recorded_git == git
                && recorded_revision == &selector
                && recorded_subdir == subdir =>
            {
                Some((commit.clone(), *tree_sha256))
            }
            Some(_) => {
                return Err(ModuleRootError::DeclarationChanged {
                    alias: alias.to_owned(),
                });
            }
            None => None,
        };
        let vendor = if recorded.is_some() {
            self.vendor
        } else {
            None
        };
        let (commit, digest) = match recorded {
            Some(recorded) => recorded,
            None if self.mode.may_write_lock() => {
                let (commit, digest) = self.fetch(alias, git, revision, subdir)?;
                self.lock.modules.insert(
                    alias.to_owned(),
                    LockedModule::Git {
                        git: git.to_owned(),
                        revision: selector,
                        subdir: subdir.to_path_buf(),
                        commit: commit.clone(),
                        tree_sha256: digest,
                    },
                );
                (commit, digest)
            }
            None => {
                return Err(ModuleRootError::NotLocked {
                    alias: alias.to_owned(),
                });
            }
        };
        let root = select_root(self.site_root, self.mode, vendor, alias, &commit, subdir)?;
        if !root.exists() {
            let pinned = RemoteRevision::Rev(commit);
            let (_, found) = self.fetch(alias, git, Some(&pinned), subdir)?;
            if found != digest {
                return Err(ModuleRootError::DigestMismatch {
                    alias: alias.to_owned(),
                    path: root,
                    expected: digest,
                    found,
                });
            }
        }
        verify_tree(alias, &root, digest)?;
        Ok(root)
    }

    fn tarball(&mut self, alias: &str, source: &TarballSource) -> Result<PathBuf, ModuleRootError> {
        ensure_active(self.cancellation)?;
        let recorded = match self.lock.modules.get(alias) {
            Some(LockedModule::Tarball {
                tarball,
                tarball_sha256,
                tree_sha256,
            }) if tarball == &source.location
                && source
                    .sha256
                    .is_none_or(|expected| expected == *tarball_sha256) =>
            {
                Some((*tarball_sha256, *tree_sha256))
            }
            Some(_) => {
                return Err(ModuleRootError::DeclarationChanged {
                    alias: alias.to_owned(),
                });
            }
            None if !self.mode.may_write_lock() => {
                return Err(ModuleRootError::NotLocked {
                    alias: alias.to_owned(),
                });
            }
            None => None,
        };
        if let Some((digest, tree)) = recorded {
            if let Some(root) = frozen_root(self.site_root, self.mode, self.vendor, alias)? {
                verify_tree(alias, &root, tree)?;
                return Ok(root);
            }
            if self.mode.may_read_cache() {
                let root = tarball_cache(self.site_root, digest);
                if root.exists() {
                    verify_tree(alias, &root, tree)?;
                    return Ok(root);
                }
            } else if matches!(source.location, TarballLocation::Url { .. }) {
                return Err(ModuleRootError::VendorMissing {
                    alias: alias.to_owned(),
                    path: self
                        .vendor
                        .unwrap_or(Path::new("vendor"))
                        .join("modules")
                        .join(alias),
                });
            }
        }
        let result = self.materialize_tarball(alias, source, recorded);
        ensure_active(self.cancellation)?;
        let (root, digest, tree) = result?;
        if recorded.is_none() {
            self.lock.modules.insert(
                alias.to_owned(),
                LockedModule::Tarball {
                    tarball: source.location.clone(),
                    tarball_sha256: digest,
                    tree_sha256: tree,
                },
            );
        }
        Ok(root)
    }

    fn materialize_tarball(
        &self,
        alias: &str,
        source: &TarballSource,
        recorded: Option<(Sha256Digest, Sha256Digest)>,
    ) -> Result<(PathBuf, Sha256Digest, Sha256Digest), ModuleRootError> {
        let failure = |source: anyhow::Error| ModuleRootError::Tarball {
            alias: alias.to_owned(),
            source,
        };
        let download = match &source.location {
            TarballLocation::Url { url } => {
                if !self.mode.may_use_network() {
                    return Err(ModuleRootError::NetworkDenied {
                        alias: alias.to_owned(),
                        scope: self.mode.scope().as_str(),
                    });
                }
                crate::archive::from_url(url, self.cancellation)
            }
            TarballLocation::File { path } => {
                let path = self.site_root.join(path);
                if self.mode.scope() == InputScope::Pure {
                    let canonical =
                        path.canonicalize()
                            .map_err(|source| ModuleRootError::Root {
                                alias: alias.to_owned(),
                                path: path.clone(),
                                source,
                            })?;
                    if !canonical.starts_with(self.site_root) {
                        return Err(ModuleRootError::OutsideSite { path });
                    }
                }
                crate::archive::from_file(&path, self.cancellation)
            }
        };
        ensure_active(self.cancellation)?;
        let mut download = download.map_err(failure)?;
        let expected = source.sha256.or(recorded.map(|(digest, _)| digest));
        if let Some(expected) = expected
            && expected != download.digest
        {
            return Err(ModuleRootError::TarballDigestMismatch {
                alias: alias.to_owned(),
                expected,
                found: download.digest,
            });
        }
        let destination = tarball_cache(self.site_root, download.digest);
        let parent = destination.parent().expect("module cache has a parent");
        std::fs::create_dir_all(parent)
            .context("could not prepare the module cache")
            .map_err(failure)?;
        let stage = tola_fs::DirectoryReplacement::new(&destination)
            .context("could not stage the module directory")
            .map_err(failure)?;
        let root = stage.staged();
        crate::archive::extract(download.file.as_file_mut(), &root, self.cancellation)
            .map_err(failure)?;
        let manifest = crate::manifest::read_one(
            &root,
            alias,
            root.clone(),
            InputMode::new(InputScope::Online, false),
        )
        .map_err(|error| failure(error.into()))?;
        crate::check::check_loaded(&manifest).map_err(failure)?;
        let tree = tree_digest(alias, &root)?;
        if let Some((_, expected)) = recorded
            && expected != tree
        {
            return Err(ModuleRootError::DigestMismatch {
                alias: alias.to_owned(),
                path: destination,
                expected,
                found: tree,
            });
        }
        ensure_active(self.cancellation)?;
        stage
            .publish()
            .context("could not publish the module cache")
            .map_err(failure)?;
        Ok((destination, download.digest, tree))
    }

    /// Fetch one module revision into the resolution cache and digest the directory it reads.
    fn fetch(
        &self,
        alias: &str,
        git: &str,
        revision: Option<&RemoteRevision>,
        subdir: &Path,
    ) -> Result<(String, Sha256Digest), ModuleRootError> {
        if !self.mode.may_use_network() {
            return Err(ModuleRootError::NetworkDenied {
                alias: alias.to_owned(),
                scope: self.mode.scope().as_str(),
            });
        }
        let staging = stage(self.site_root)?;
        let commit = acquire(alias, git, revision, &staging, self.cancellation)?;
        if let Some(RemoteRevision::Rev(expected)) = revision
            && expected != &commit
        {
            return Err(ModuleRootError::CommitMismatch {
                alias: alias.to_owned(),
                expected: expected.clone(),
                found: commit,
            });
        }
        let root = select_subdir(alias, &staging.source(), subdir)?;
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
fn select_subdir(alias: &str, snapshot: &Path, subdir: &Path) -> Result<PathBuf, ModuleRootError> {
    let declared = snapshot.join(subdir);
    let root = declared
        .canonicalize()
        .map_err(|source| ModuleRootError::Root {
            alias: alias.to_owned(),
            path: declared,
            source,
        })?;
    let snapshot = snapshot
        .canonicalize()
        .map_err(|source| ModuleRootError::Root {
            alias: alias.to_owned(),
            path: snapshot.to_path_buf(),
            source,
        })?;
    if !root.starts_with(&snapshot) || !root.is_dir() {
        return Err(ModuleRootError::Subdir {
            alias: alias.to_owned(),
        });
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
    alias: &str,
    commit: &str,
    subdir: &Path,
) -> Result<PathBuf, ModuleRootError> {
    if let Some(frozen) = frozen_root(site_root, mode, vendor, alias)? {
        return Ok(frozen);
    }
    if mode.scope() == InputScope::Pure {
        return Err(ModuleRootError::VendorMissing {
            alias: alias.to_owned(),
            path: vendor
                .unwrap_or(Path::new("vendor"))
                .join("modules")
                .join(alias),
        });
    }
    Ok(cache_root(site_root, commit, subdir))
}

fn frozen_root(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    alias: &str,
) -> Result<Option<PathBuf>, ModuleRootError> {
    let Some(root) = vendor
        .map(|path| path.join("modules").join(alias))
        .filter(|path| path.is_dir())
    else {
        return Ok(None);
    };
    if mode.scope() == InputScope::Pure {
        let canonical = root
            .canonicalize()
            .map_err(|source| ModuleRootError::Root {
                alias: alias.to_owned(),
                path: root.clone(),
                source,
            })?;
        if !canonical.starts_with(site_root) {
            return Err(ModuleRootError::OutsideSite { path: root });
        }
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

/// Why one declared module has no readable directory.
#[derive(Debug, thiserror::Error)]
pub enum ModuleRootError {
    #[error("Tola could not obtain the tarball for module `{alias}`: {source}")]
    Tarball {
        alias: String,
        #[source]
        source: anyhow::Error,
    },
    #[error(
        "the tarball for module `{alias}` does not match its checksum: expected {expected}, found {found}"
    )]
    TarballDigestMismatch {
        alias: String,
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
    #[error("the `subdir` of module `{alias}` does not exist inside the read revision")]
    Subdir { alias: String },
    #[error("Tola could not fetch module `{alias}`: {source}")]
    Fetch {
        alias: String,
        #[source]
        source: crate::remote::FetchError,
    },
    #[error("Tola could not extract module `{alias}` from its revision: {source}")]
    Extract {
        alias: String,
        #[source]
        source: crate::checkout::CheckoutError,
    },
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
    fn locked_tarballs_rebuild_caches_verify_changes_and_use_vendor() {
        let site = tempfile::tempdir().unwrap();
        let package = site.path().join("theme.tar.gz");
        write_package(&package, b"#let value = 1");
        let declarations = BTreeMap::from([(
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
        let cancellation = BundleCancellation::default();
        let offline = InputMode::new(InputScope::Offline, false);
        let locked = InputMode::new(InputScope::Online, true);
        let pure = InputMode::new(InputScope::Pure, false);
        let roots =
            module_roots_with_bundle(site.path(), offline, None, &declarations, &cancellation)
                .unwrap();
        let original = InputLock::read(site.path()).unwrap().unwrap();
        fs::write(roots["theme"].join("lib.typ"), "tampered cache").unwrap();
        assert!(matches!(
            module_roots_with_bundle(
                site.path(),
                InputMode::new(InputScope::Online, false),
                None,
                &declarations,
                &cancellation
            ),
            Err(ModuleRootError::DigestMismatch { .. })
        ));
        module_roots_with_bundle(site.path(), pure, None, &declarations, &cancellation).unwrap();
        fs::remove_dir_all(&roots["theme"]).unwrap();
        module_roots_with_bundle(site.path(), locked, None, &declarations, &cancellation).unwrap();
        assert_eq!(InputLock::read(site.path()).unwrap().unwrap(), original);
        write_package(&package, b"#let value = 2");
        fs::remove_dir_all(&roots["theme"]).unwrap();
        assert!(matches!(
            module_roots_with_bundle(site.path(), locked, None, &declarations, &cancellation),
            Err(ModuleRootError::TarballDigestMismatch { .. })
        ));
        assert_eq!(InputLock::read(site.path()).unwrap().unwrap(), original);
        let mut updated = original.clone();
        let roots = refresh_module_roots_with_bundle(
            site.path(),
            offline,
            None,
            &declarations,
            &mut updated,
            &cancellation,
        )
        .unwrap();
        assert_ne!(updated, original);
        updated.commit(site.path(), &original).unwrap();
        let vendor = site.path().join("vendor");
        let frozen = vendor.join("modules/theme");
        fs::create_dir_all(&frozen).unwrap();
        for entry in fs::read_dir(&roots["theme"]).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), frozen.join(entry.file_name())).unwrap();
        }
        fs::remove_file(package).unwrap();
        fs::remove_dir_all(&roots["theme"]).unwrap();
        let roots = module_roots_with_bundle(
            site.path(),
            pure,
            Some(&vendor),
            &declarations,
            &cancellation,
        )
        .unwrap();
        assert_eq!(roots["theme"], frozen);
    }
}
