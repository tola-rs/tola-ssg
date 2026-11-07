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
use crate::modules::{ModuleDeclaration, ModuleSource, RemoteRevision, remote};

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
    let mut roots = BTreeMap::new();
    for (alias, declaration) in declarations {
        cancellation.ensure_active()?;
        let root = match &declaration.source {
            ModuleSource::Local { path } => {
                let root = local_root(site_root, mode, alias, path)?;
                record_local(site_root, lock, alias, path, &root)?;
                root
            }
            ModuleSource::Remote {
                git,
                revision,
                subdir,
            } => remote_root(
                site_root,
                mode,
                vendor,
                lock,
                alias,
                git,
                revision.as_ref(),
                subdir,
                cancellation,
            )?,
        };
        roots.insert(alias.clone(), root);
    }
    Ok(roots)
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

/// A local module's directory, which must stay inside the site.
fn local_root(
    site_root: &Path,
    mode: InputMode,
    alias: &str,
    path: &Path,
) -> Result<PathBuf, ModuleRootError> {
    let declared = site_root.join(path);
    if !declared.is_dir() {
        return Err(ModuleRootError::NotDirectory {
            alias: alias.to_owned(),
            path: declared,
        });
    }
    if mode.scope() == InputScope::Pure {
        let canonical = declared
            .canonicalize()
            .map_err(|source| ModuleRootError::Root {
                alias: alias.to_owned(),
                path: declared.clone(),
                source,
            })?;
        if !canonical.starts_with(site_root) {
            return Err(ModuleRootError::OutsideSite { path: canonical });
        }
    }
    Ok(declared)
}

/// Record what a local module read, which only proves the bytes a build saw.
fn record_local(
    site_root: &Path,
    lock: &mut InputLock,
    alias: &str,
    path: &Path,
    root: &Path,
) -> Result<(), ModuleRootError> {
    let tree_sha256 = tree_sha256(root).map_err(|source| ModuleRootError::Tree {
        alias: alias.to_owned(),
        path: root.to_path_buf(),
        source,
    })?;
    let recorded = path
        .strip_prefix(site_root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf());
    let entry = LockedModule::Local {
        path: recorded,
        tree_sha256,
    };
    lock.modules.insert(alias.to_owned(), entry);
    Ok(())
}

/// A remote module's directory, fetched when this attempt may write and the lock has no fact yet.
#[allow(clippy::too_many_arguments)]
fn remote_root(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    lock: &mut InputLock,
    alias: &str,
    git: &str,
    revision: Option<&RemoteRevision>,
    subdir: &Path,
    cancellation: &BuildCancellation,
) -> Result<PathBuf, ModuleRootError> {
    let selector = locked_revision(revision);
    let recorded = match lock.modules.get(alias) {
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
        None if mode.may_write_lock() => {
            let (commit, digest) = fetch(site_root, alias, git, revision, subdir, cancellation)?;
            lock.modules.insert(
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
    let root = select_root(site_root, mode, vendor, alias, &commit, subdir)?;
    if mode.requires_lock() {
        let found = tree_sha256(&root).map_err(|source| ModuleRootError::Tree {
            alias: alias.to_owned(),
            path: root.clone(),
            source,
        })?;
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
    site_root: &Path,
    alias: &str,
    git: &str,
    revision: Option<&RemoteRevision>,
    subdir: &Path,
    cancellation: &BuildCancellation,
) -> Result<(String, Sha256Digest), ModuleRootError> {
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
    let stage = tempfile::tempdir_in(&internal).map_err(|source| ModuleRootError::Io {
        action: "stage",
        path: internal,
        source,
    })?;
    let fetched = remote::fetch(git, revision, stage.path(), cancellation).map_err(|source| {
        ModuleRootError::Fetch {
            alias: alias.to_owned(),
            source,
        }
    })?;
    let root =
        fetched
            .root
            .join(subdir)
            .canonicalize()
            .map_err(|source| ModuleRootError::Root {
                alias: alias.to_owned(),
                path: fetched.root.join(subdir),
                source,
            })?;
    let snapshot = fetched
        .root
        .canonicalize()
        .map_err(|source| ModuleRootError::Root {
            alias: alias.to_owned(),
            path: fetched.root.clone(),
            source,
        })?;
    if !root.starts_with(&snapshot) || !root.is_dir() {
        return Err(ModuleRootError::Subdir {
            alias: alias.to_owned(),
        });
    }
    let digest = tree_sha256(&root).map_err(|source| ModuleRootError::Tree {
        alias: alias.to_owned(),
        path: root,
        source,
    })?;
    let destination = parent.join(&fetched.commit);
    if !destination.exists() {
        std::fs::rename(&fetched.root, &destination).map_err(|source| ModuleRootError::Io {
            action: "store",
            path: destination,
            source,
        })?;
    }
    Ok((fetched.commit, digest))
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
    let frozen = vendor.map(|vendor| vendor.join("modules").join(alias));
    if let Some(frozen) = &frozen
        && frozen.is_dir()
    {
        return Ok(frozen.clone());
    }
    if mode.scope() == InputScope::Pure {
        return Err(ModuleRootError::VendorMissing {
            alias: alias.to_owned(),
            path: frozen.unwrap_or_else(|| PathBuf::from("vendor/modules").join(alias)),
        });
    }
    let snapshot = layout::modules(site_root).join(alias).join(commit);
    let root = snapshot
        .join(subdir)
        .canonicalize()
        .map_err(|source| ModuleRootError::Root {
            alias: alias.to_owned(),
            path: snapshot.join(subdir),
            source,
        })?;
    let snapshot = snapshot
        .canonicalize()
        .map_err(|source| ModuleRootError::Root {
            alias: alias.to_owned(),
            path: snapshot,
            source,
        })?;
    if !root.starts_with(&snapshot) || !root.is_dir() {
        return Err(ModuleRootError::Subdir {
            alias: alias.to_owned(),
        });
    }
    Ok(root)
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
    #[error("Tola could not fetch module `{alias}`")]
    Fetch {
        alias: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("Tola could not {action} `{path}`")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
