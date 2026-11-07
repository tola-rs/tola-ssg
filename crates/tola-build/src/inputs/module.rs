//! The directory each declared module reads, and the lock facts that fix it.
//!
//! A local module reads its site directory. A remote module reads the frozen copy below
//! `vendor/`, or the snapshot below `.tola/modules` when no frozen copy exists. Only this module
//! may touch the network, and only an attempt that may write the lock records what it found.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::InputScope;
use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::filesystem::{INTERNAL_DIR, layout};
use crate::inputs::{
    InputLock, InputMode, LockError, LockedModule, LockedRevision, Sha256Digest, TreeHashError,
    tree_sha256,
};
use crate::modules::{ModuleDeclaration, ModuleSource, RemoteRevision, checkout, remote};

/// The directory every declared module reads.
///
/// A local module must be a directory inside the site. A remote module reads the frozen copy below
/// `vendor/` when one exists and the verified snapshot below `.tola/modules` otherwise. The lock
/// records each resolved fact, and is replaced only when this attempt may write it.
pub fn module_roots(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    cancellation: &BuildCancellation,
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    let site_root = site_root
        .canonicalize()
        .map_err(|source| ModuleRootError::SiteRoot { source })?;
    let mut lock = InputLock::read(&site_root)?.unwrap_or_default();
    let original = lock.clone();
    let roots = record_module_roots(
        &site_root,
        mode,
        vendor,
        declarations,
        &mut lock,
        cancellation,
    )?;
    if lock != original && mode.may_write_lock() {
        lock.write_atomic(&site_root)?;
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
    cancellation: &BuildCancellation,
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    ModuleResolution {
        site_root,
        mode,
        vendor,
        lock,
        cancellation,
    }
    .record(declarations)
}

/// Forget the fact of every selected module, then resolve and record it again.
///
/// Refreshing a module starts by dropping what the lock knew, so the next resolution asks the
/// repository for the declared selector instead of reusing the recorded commit.
pub fn refresh_module_roots(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    lock: &mut InputLock,
    cancellation: &BuildCancellation,
) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    for alias in declarations.keys() {
        lock.modules.remove(alias);
    }
    record_module_roots(site_root, mode, vendor, declarations, lock, cancellation)
}

/// The directory one declaration points at, without fetching or verifying it.
///
/// Use this for observation and watch paths. A build reads module roots through [`module_roots`].
pub fn declaration_root(
    site_root: &Path,
    vendor: Option<&Path>,
    alias: &str,
    source: &ModuleSource,
) -> Result<PathBuf, ModuleRootError> {
    match source {
        ModuleSource::Local { path } => Ok(site_root.join(path)),
        ModuleSource::Remote {
            git,
            revision,
            subdir,
        } => {
            let lock = InputLock::read(site_root)?.unwrap_or_default();
            let selector = locked_revision(revision.as_ref());
            match lock.modules.get(alias) {
                Some(LockedModule::Remote {
                    git: recorded_git,
                    revision: recorded_revision,
                    subdir: recorded_subdir,
                    commit,
                    ..
                }) if recorded_git == git
                    && recorded_revision == &selector
                    && recorded_subdir == subdir =>
                {
                    select_root(
                        site_root,
                        InputMode::new(InputScope::Online, false),
                        vendor,
                        alias,
                        commit,
                        subdir,
                    )
                }
                _ => Err(ModuleRootError::NotLocked {
                    alias: alias.to_owned(),
                }),
            }
        }
    }
}

/// The site, mode, frozen copy, lock, and cancellation one attempt resolves module roots under.
struct ModuleResolution<'a> {
    site_root: &'a Path,
    mode: InputMode,
    vendor: Option<&'a Path>,
    lock: &'a mut InputLock,
    cancellation: &'a BuildCancellation,
}

impl ModuleResolution<'_> {
    /// Record the root and digest of every declaration into the lock.
    fn record(
        &mut self,
        declarations: &BTreeMap<String, ModuleDeclaration>,
    ) -> Result<BTreeMap<String, PathBuf>, ModuleRootError> {
        let mut roots = BTreeMap::new();
        for (alias, declaration) in declarations {
            self.cancellation.ensure_active()?;
            let root = self.resolve(alias, declaration)?;
            roots.insert(alias.clone(), root);
        }
        Ok(roots)
    }

    /// The directory one declaration reads, and the fact the lock records for it.
    fn resolve(
        &mut self,
        alias: &str,
        declaration: &ModuleDeclaration,
    ) -> Result<PathBuf, ModuleRootError> {
        match &declaration.source {
            ModuleSource::Local { path } => self.local(alias, path),
            ModuleSource::Remote {
                git,
                revision,
                subdir,
            } => self.remote(alias, git, revision.as_ref(), subdir),
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
            Some(LockedModule::Remote {
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
        let (commit, digest) = match recorded {
            Some(recorded) => recorded,
            None if self.mode.may_write_lock() => {
                let (commit, digest) = self.fetch(alias, git, revision, subdir)?;
                self.lock.modules.insert(
                    alias.to_owned(),
                    LockedModule::Remote {
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
        let root = select_root(
            self.site_root,
            self.mode,
            self.vendor,
            alias,
            &commit,
            subdir,
        )?;
        if self.mode.requires_lock() {
            let found = tree_digest(alias, &root)?;
            if found != digest {
                return Err(ModuleRootError::DigestMismatch {
                    alias: alias.to_owned(),
                    path: root,
                    expected: digest,
                    found,
                });
            }
        }
        Ok(root)
    }

    /// Fetch one module revision into the resolution cache and digest the directory it reads.
    fn fetch(
        &self,
        alias: &str,
        git: &str,
        revision: Option<&RemoteRevision>,
        subdir: &Path,
    ) -> Result<(String, Sha256Digest), ModuleRootError> {
        let staging = stage(self.site_root, alias)?;
        let commit = acquire(alias, git, revision, &staging, self.cancellation)?;
        let root = select_subdir(alias, &staging.source(), subdir)?;
        let digest = tree_digest(alias, &root)?;
        store(&staging, &commit)?;
        Ok((commit, digest))
    }
}

/// One fetch attempt's staging area, removed unless its source tree moves into the cache.
struct Staging {
    /// The module cache directory that holds one snapshot per commit.
    parent: PathBuf,
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
fn stage(site_root: &Path, alias: &str) -> Result<Staging, ModuleRootError> {
    let parent = layout::modules(site_root).join(alias);
    std::fs::create_dir_all(&parent).map_err(|source| ModuleRootError::Io {
        action: "create the module cache for",
        path: parent.clone(),
        source,
    })?;
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
    Ok(Staging { parent, directory })
}

/// Fetch the declared revision and materialize its tree into the staging area.
fn acquire(
    alias: &str,
    git: &str,
    revision: Option<&RemoteRevision>,
    staging: &Staging,
    cancellation: &BuildCancellation,
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
            cancellation.ensure_active()?;
            return Err(ModuleRootError::Fetch {
                alias: alias.to_owned(),
                source,
            });
        }
    };
    cancellation.ensure_active()?;
    let root = staging.source();
    if let Err(source) =
        checkout::materialize(&fetched.repository, fetched.tree, &root, cancellation)
    {
        cancellation.ensure_active()?;
        return Err(ModuleRootError::Extract {
            alias: alias.to_owned(),
            source,
        });
    }
    cancellation.ensure_active()?;
    Ok(fetched.commit)
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

/// Move the staged source tree into the cache under its commit.
fn store(staging: &Staging, commit: &str) -> Result<(), ModuleRootError> {
    let destination = staging.parent.join(commit);
    if !destination.exists() {
        std::fs::rename(staging.source(), &destination).map_err(|source| ModuleRootError::Io {
            action: "store",
            path: destination,
            source,
        })?;
    }
    Ok(())
}

/// The directory a remote module reads: the frozen copy, then the verified cache.
fn select_root(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    alias: &str,
    commit: &str,
    subdir: &Path,
) -> Result<PathBuf, ModuleRootError> {
    root_strategy(site_root, mode, vendor, alias, commit)?.materialize(alias, subdir)
}

/// Which copy of a remote module a resolution reads.
enum RootStrategy {
    /// The frozen copy below `vendor/modules/<alias>`.
    Frozen(PathBuf),
    /// The commit snapshot below `.tola/modules/<alias>`.
    Cached(PathBuf),
}

impl RootStrategy {
    /// The directory this copy makes readable, validated against `subdir`.
    fn materialize(self, alias: &str, subdir: &Path) -> Result<PathBuf, ModuleRootError> {
        match self {
            Self::Frozen(root) => Ok(root),
            Self::Cached(snapshot) => select_subdir(alias, &snapshot, subdir),
        }
    }
}

/// Decide which copy of a remote module resolution reads.
fn root_strategy(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    alias: &str,
    commit: &str,
) -> Result<RootStrategy, ModuleRootError> {
    let frozen = vendor.map(|vendor| vendor.join("modules").join(alias));
    if let Some(frozen) = &frozen
        && frozen.is_dir()
    {
        return Ok(RootStrategy::Frozen(frozen.clone()));
    }
    if mode.scope() == InputScope::Pure {
        return Err(ModuleRootError::VendorMissing {
            alias: alias.to_owned(),
            path: frozen.unwrap_or_else(|| PathBuf::from("vendor/modules").join(alias)),
        });
    }
    Ok(RootStrategy::Cached(
        layout::modules(site_root).join(alias).join(commit),
    ))
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
    #[error(transparent)]
    Cancelled(#[from] BuildCancelled),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error("Tola could not resolve the site root")]
    SiteRoot {
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not resolve `{path}` for module `{alias}`")]
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
    #[error("the `subdir` of module `{alias}` does not exist inside the resolved revision")]
    Subdir { alias: String },
    #[error("Tola could not fetch module `{alias}`: {source}")]
    Fetch {
        alias: String,
        #[source]
        source: crate::modules::FetchError,
    },
    #[error("Tola could not extract module `{alias}` from its revision: {source}")]
    Extract {
        alias: String,
        #[source]
        source: crate::modules::CheckoutError,
    },
    #[error("Tola could not {action} `{path}`")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
