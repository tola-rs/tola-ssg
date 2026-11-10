//! Whole-tree replacement through a transient workspace beside the target.
//!
//! A swap stages the trees it will install, journals their destinations once the caller has
//! verified them, and keeps each replaced tree until the replacement completes.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::atomic_write;
use super::path::path_failure_reason;
use tola_diagnostic::{display_path, render_relative};

const OWNER_FILE: &str = "owner";
const OWNER_HEADER: &str = "tola-swap";
const COMMIT_JOURNAL: &str = "commit";
const PREVIOUS_DIRECTORY: &str = "previous";
const STAGED_DIRECTORY: &str = "staged";

/// The transient workspace beside `target` that a swap of it prepares in.
///
/// `None` when `target` names no directory entry, so it has no sibling name.
pub fn swap_workspace(target: &Path) -> Option<PathBuf> {
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name()?);
    name.push("-swap");
    Some(target.with_file_name(name))
}

/// One tree a swap installs: `staged` replaces `destination`.
#[derive(Debug)]
pub struct SwapMember {
    pub staged: PathBuf,
    pub destination: PathBuf,
}

/// The replacement of one site tree through a transient workspace beside it.
#[derive(Debug)]
pub struct TreeSwap {
    site_root: PathBuf,
    target: PathBuf,
    workspace: PathBuf,
}

impl TreeSwap {
    /// A swap of `target`, whose workspace record names it relative to `site_root`.
    pub fn beside(site_root: &Path, target: &Path) -> Result<Self, SwapError> {
        relative_target(site_root, target)?;
        let workspace = swap_workspace(target).ok_or(SwapError::TargetOutsideSite)?;
        Ok(Self {
            site_root: site_root.to_path_buf(),
            target: target.to_path_buf(),
            workspace,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Where the caller stages member `index`; the journal records destinations in this order.
    pub fn staged(&self, index: usize) -> PathBuf {
        self.workspace
            .join(STAGED_DIRECTORY)
            .join(index.to_string())
    }

    /// The directory holding one replaced tree per committed member index.
    fn previous(&self) -> PathBuf {
        self.workspace.join(PREVIOUS_DIRECTORY)
    }

    /// Create the workspace and record this target's ownership of it.
    ///
    /// Only an empty directory, or one this target already recorded, is taken over: a workspace
    /// holding another writer's files is never adopted or deleted.
    pub fn claim(&self) -> Result<(), SwapError> {
        fs::create_dir_all(
            self.workspace
                .parent()
                .expect("a swap workspace has a parent"),
        )
        .map_err(|source| io_error("could not create the replacement workspace", source))?;
        match fs::create_dir(&self.workspace) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(io_error(
                    "could not create the replacement workspace",
                    source,
                ));
            }
        }
        if self.is_claimed()? {
            return Ok(());
        }
        if !super::directory_is_empty(&self.workspace)
            .map_err(|source| io_error("could not read the replacement workspace", source))?
        {
            return Err(self.foreign_workspace());
        }
        atomic_write(&self.owner_file(), &self.owner_record()).map_err(|source| {
            io_error(
                "could not record the replacement workspace ownership",
                source,
            )
        })
    }

    /// Whether this workspace holds the record that it belongs to this target.
    pub fn is_claimed(&self) -> Result<bool, SwapError> {
        let mut record = match tola_fs::open_regular_file(&self.owner_file(), false) {
            Ok(record) => record,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(io_error(
                    "could not read the replacement workspace ownership",
                    source,
                ));
            }
        };
        let mut contents = Vec::new();
        record.read_to_end(&mut contents).map_err(|source| {
            io_error("could not read the replacement workspace ownership", source)
        })?;
        Ok(contents == self.owner_record())
    }

    /// Discard a replacement that never committed, or complete one that did.
    ///
    /// A journal is rolled forward: the caller verified every staged tree before the journal was
    /// written, so staged bytes are authoritative and a member is only ever filled from its own
    /// staged tree. Rolling back could leave a destination absent once its staged tree is gone.
    pub fn recover(&self) -> Result<(), SwapError> {
        match fs::symlink_metadata(&self.workspace) {
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(source) => {
                return Err(io_error("could not read the replacement workspace", source));
            }
            Ok(_) => {}
        }
        self.claim()?;
        let journal = match fs::read_to_string(self.journal_file()) {
            Ok(journal) => journal,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                remove_tree(&self.workspace.join(STAGED_DIRECTORY))?;
                return Ok(());
            }
            Err(source) => {
                return Err(io_error(
                    "could not read the interrupted replacement",
                    source,
                ));
            }
        };
        let previous = self.previous();
        fs::create_dir_all(&previous)
            .map_err(|source| io_error("could not create the replacement workspace", source))?;
        for (index, destination) in journal.lines().enumerate() {
            let destination = self.site_root.join(destination);
            let staged = self.staged(index);
            if staged.is_dir() {
                if destination.exists() {
                    rename(
                        &destination,
                        &previous.join(index.to_string()),
                        "could not move the previous tree aside",
                    )?;
                }
                rename(
                    &staged,
                    &destination,
                    "could not complete the interrupted replacement",
                )?;
            } else if !destination.exists() {
                return Err(SwapError::Unrecoverable {
                    site_root: self.site_root.clone(),
                    destination,
                    workspace: self.workspace.clone(),
                });
            }
        }
        remove_tree(&previous)?;
        fs::remove_file(self.journal_file())
            .map_err(|source| io_error("could not finish the interrupted replacement", source))?;
        Ok(())
    }

    /// Replace every member's destination with its staged tree.
    ///
    /// The journal lands before any destination moves, so a failure part-way leaves the commit
    /// for [`Self::recover`] rather than a destination absent with nothing recorded. Every
    /// staged tree is checked first: a destination must never move aside for a tree that does
    /// not exist.
    pub fn commit(&self, members: &[SwapMember]) -> Result<(), SwapError> {
        if let Some(member) = members.iter().find(|member| !member.staged.is_dir()) {
            return Err(SwapError::MissingStaged {
                site_root: self.site_root.clone(),
                destination: member.destination.clone(),
            });
        }
        let mut journal = String::new();
        for member in members {
            let relative = relative_target(&self.site_root, &member.destination)?;
            journal.push_str(&render_relative(relative));
            journal.push('\n');
        }
        atomic_write(&self.journal_file(), journal.as_bytes())
            .map_err(|source| io_error("could not record the replacement", source))?;
        let previous = self.previous();
        fs::create_dir_all(&previous)
            .map_err(|source| io_error("could not create the replacement workspace", source))?;
        for (index, member) in members.iter().enumerate() {
            if !member.destination.exists() {
                continue;
            }
            rename(
                &member.destination,
                &previous.join(index.to_string()),
                "could not move the previous tree aside",
            )?;
        }
        for member in members {
            rename(
                &member.staged,
                &member.destination,
                "could not install the replacement tree",
            )?;
        }
        Ok(())
    }

    /// Remove the workspace once its replacement has completed.
    ///
    /// A workspace this target did not record is refused: its files are another writer's.
    pub fn finish(&self) -> Result<(), SwapError> {
        match fs::symlink_metadata(&self.workspace) {
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(io_error("could not read the replacement workspace", source));
            }
            Ok(_) => {}
        }
        if !self.is_claimed()? {
            return Err(self.foreign_workspace());
        }
        fs::remove_dir_all(&self.workspace)
            .map_err(|source| io_error("could not clean up the replacement workspace", source))
    }

    fn owner_file(&self) -> PathBuf {
        self.workspace.join(OWNER_FILE)
    }

    fn journal_file(&self) -> PathBuf {
        self.workspace.join(COMMIT_JOURNAL)
    }

    // Site-root-relative, so the record names the same tree wherever the site lives.
    fn owner_record(&self) -> Vec<u8> {
        let target = relative_target(&self.site_root, &self.target)
            .expect("a swap keeps its target below the site root");
        format!("{OWNER_HEADER}\ntarget={}\n", render_relative(target)).into_bytes()
    }

    fn foreign_workspace(&self) -> SwapError {
        SwapError::ForeignWorkspace {
            site_root: self.site_root.clone(),
            workspace: self.workspace.clone(),
        }
    }
}

/// The site-root-relative spelling of a tree a replacement may own.
fn relative_target<'a>(site_root: &Path, path: &'a Path) -> Result<&'a Path, SwapError> {
    match path.strip_prefix(site_root) {
        Ok(relative) if !relative.as_os_str().is_empty() => Ok(relative),
        _ => Err(SwapError::TargetOutsideSite),
    }
}

fn rename(from: &Path, to: &Path, operation: &'static str) -> Result<(), SwapError> {
    fs::rename(from, to).map_err(|source| io_error(operation, source))
}

/// Remove a tree the swap created, tolerating its absence.
fn remove_tree(path: &Path) -> Result<(), SwapError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(
            "could not clean up the replacement workspace",
            source,
        )),
    }
}

fn io_error(operation: &'static str, source: io::Error) -> SwapError {
    SwapError::Io { operation, source }
}

/// Why a whole-tree replacement could not finish.
#[derive(Debug, Error)]
pub enum SwapError {
    /// The target has no sibling workspace, or is not a directory below the site root.
    #[error("a replacement target must be a directory below the site root")]
    TargetOutsideSite,

    /// The workspace holds another writer's files, or records another target.
    #[error(
        "the replacement workspace `{}` is not owned by this target; move it aside and try again",
        display_path(.workspace, .site_root)
    )]
    ForeignWorkspace {
        site_root: PathBuf,
        workspace: PathBuf,
    },

    /// A member names no staged tree, so moving its destination aside would lose it.
    #[error(
        "the replacement for `{}` is missing; nothing was replaced",
        display_path(.destination, .site_root)
    )]
    MissingStaged {
        site_root: PathBuf,
        destination: PathBuf,
    },

    /// A committed member has neither its destination nor its staged tree.
    #[error(
        "the interrupted replacement of `{}` cannot be completed; delete `{}` and try again",
        display_path(.destination, .site_root),
        display_path(.workspace, .site_root)
    )]
    Unrecoverable {
        site_root: PathBuf,
        destination: PathBuf,
        workspace: PathBuf,
    },

    #[error("{operation}: {}", path_failure_reason(.source))]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn vendor_site() -> (TempDir, PathBuf) {
        let site = TempDir::new().unwrap();
        let vendor = site.path().join("vendor");
        fs::create_dir_all(vendor.join("packages/old")).unwrap();
        fs::create_dir_all(vendor.join("icons")).unwrap();
        fs::write(vendor.join("README.md"), "author file").unwrap();
        fs::write(vendor.join("icons/old.json"), "old icons").unwrap();
        (site, vendor)
    }

    fn staged(swap: &TreeSwap, index: usize, files: &[(&str, &str)]) -> PathBuf {
        let staged = swap.staged(index);
        fs::create_dir_all(&staged).unwrap();
        for (name, contents) in files {
            let path = staged.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        staged
    }

    #[test]
    fn commit_replaces_only_its_members() {
        let (site, vendor) = vendor_site();
        let swap = TreeSwap::beside(site.path(), &vendor).unwrap();
        swap.claim().unwrap();
        let packages = staged(&swap, 0, &[("new/typst.toml", "package")]);
        let icons = staged(&swap, 1, &[("new.json", "icons")]);

        swap.commit(&[
            SwapMember {
                staged: packages,
                destination: vendor.join("packages"),
            },
            SwapMember {
                staged: icons,
                destination: vendor.join("icons"),
            },
        ])
        .unwrap();
        swap.finish().unwrap();

        assert_eq!(
            fs::read_to_string(vendor.join("README.md")).unwrap(),
            "author file"
        );
        assert!(!vendor.join("packages/old").exists());
        assert_eq!(
            fs::read_to_string(vendor.join("packages/new/typst.toml")).unwrap(),
            "package"
        );
        assert_eq!(
            fs::read_to_string(vendor.join("icons/new.json")).unwrap(),
            "icons"
        );
        assert!(!swap.workspace().exists());
    }

    /// A destination never moves aside for a staged tree that is gone.
    #[test]
    fn commit_refuses_no_staged_tree() {
        let site = TempDir::new().unwrap();
        let target = site.path().join("public");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("index.html"), "published").unwrap();
        let swap = TreeSwap::beside(site.path(), &target).unwrap();
        swap.claim().unwrap();

        let error = swap
            .commit(&[SwapMember {
                staged: swap.staged(0),
                destination: target.clone(),
            }])
            .unwrap_err();

        assert!(matches!(error, SwapError::MissingStaged { .. }));
        assert_eq!(fs::read(target.join("index.html")).unwrap(), b"published");
    }

    /// A journal written before the first move still installs its member on recovery.
    #[test]
    fn recovery_installs_a_journaled_candidate() {
        let site = TempDir::new().unwrap();
        let target = site.path().join("public");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("index.html"), "published").unwrap();
        let swap = TreeSwap::beside(site.path(), &target).unwrap();
        swap.claim().unwrap();
        let candidate = staged(&swap, 0, &[("index.html", "candidate")]);
        // A file where replaced trees are kept stops the commit right after its journal.
        fs::write(swap.previous(), "occupied").unwrap();

        let interrupted = swap.commit(&[SwapMember {
            staged: candidate,
            destination: target.clone(),
        }]);
        assert!(interrupted.is_err());
        assert_eq!(fs::read(target.join("index.html")).unwrap(), b"published");

        fs::remove_file(swap.previous()).unwrap();
        swap.recover().unwrap();
        swap.finish().unwrap();

        assert_eq!(fs::read(target.join("index.html")).unwrap(), b"candidate");
        assert!(!swap.workspace().exists());
    }

    #[test]
    fn recovery_completes_a_half_installed_commit() {
        let site = TempDir::new().unwrap();
        let first = site.path().join("first");
        fs::create_dir(&first).unwrap();
        fs::write(first.join("index.html"), "previous").unwrap();
        let later = site.path().join("later/second");
        let swap = TreeSwap::beside(site.path(), &first).unwrap();
        swap.claim().unwrap();
        let first_staged = staged(&swap, 0, &[("index.html", "installed")]);
        let later_staged = staged(&swap, 1, &[("index.html", "installed")]);

        let interrupted = swap.commit(&[
            SwapMember {
                staged: first_staged,
                destination: first.clone(),
            },
            SwapMember {
                staged: later_staged,
                destination: later.clone(),
            },
        ]);
        assert!(interrupted.is_err());
        assert_eq!(fs::read(first.join("index.html")).unwrap(), b"installed");

        fs::create_dir_all(later.parent().unwrap()).unwrap();
        swap.recover().unwrap();
        swap.finish().unwrap();

        assert_eq!(fs::read(later.join("index.html")).unwrap(), b"installed");
        assert!(!swap.workspace().exists());
    }
}
