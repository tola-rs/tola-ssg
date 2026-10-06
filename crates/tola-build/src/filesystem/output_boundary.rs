//! The site output root and the inputs it must not overlap.
//!
//! The boundary is a site layout rule: where one site publishes, and which of its own inputs that
//! place must not contain. Validation belongs here rather than to the publisher, so configuration
//! loading can reject an unusable output root before anything is built.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::path::display_path;
use super::{
    FilesystemSourceIdentity, INTERNAL_DIR, SITE_BUILD_LOCK_FILE, canonical_site_root,
    path_failure_reason, publication_workspace, site_relative_absolute,
};

/// One site input the output root must not contain.
#[derive(Debug)]
pub(crate) struct ProtectedPath {
    pub(crate) kind: &'static str,
    pub(crate) path: PathBuf,
}

/// The resolved output root of one site, with every input it must not overlap.
#[derive(Debug)]
pub(crate) struct OutputBoundary {
    pub(crate) site_root: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) protected: Vec<ProtectedPath>,
}

impl OutputBoundary {
    /// Resolve the declared output path and check it against every protected input.
    pub(crate) fn resolve<'a>(
        site_root: &Path,
        output: &Path,
        protected: impl IntoIterator<Item = (&'static str, &'a Path)>,
    ) -> Result<Self, OutputBoundaryError> {
        let (supplied_root, site_root) = canonical_site_root(site_root)?;
        if !site_root.is_dir() {
            return Err(OutputBoundaryError::InvalidSiteRoot);
        }

        let output = site_relative_absolute(&supplied_root, &site_root, output)?;
        let workspace = publication_workspace(&output).ok_or(OutputBoundaryError::OutsideSite)?;
        let mut resolved = Vec::new();
        for (kind, path) in protected {
            resolved.push(ProtectedPath {
                kind,
                path: site_relative_absolute(&supplied_root, &site_root, path)?,
            });
        }

        let boundary = Self {
            site_root,
            output,
            workspace,
            protected: resolved,
        };
        boundary.revalidate()?;
        Ok(boundary)
    }

    pub(crate) fn revalidate(&self) -> Result<(), OutputBoundaryError> {
        if self.output == self.site_root || !self.output.starts_with(&self.site_root) {
            return Err(OutputBoundaryError::OutsideSite);
        }
        let reserved = [
            ProtectedPath {
                kind: "`.tola` directory",
                path: self.site_root.join(INTERNAL_DIR),
            },
            ProtectedPath {
                kind: "site build lock",
                path: self.site_root.join(SITE_BUILD_LOCK_FILE),
            },
        ];
        for path in [&self.output, &self.workspace] {
            // The layout rule is decided before the path walk: a publication path below `.tola`
            // or the lock is a misdeclared `build.publish-dir`, whatever kind of entry sits there.
            for protected in &reserved {
                self.validate_overlap(path, protected)?;
            }
            self.validate_path_chain(path)?;
            for protected in &self.protected {
                self.validate_overlap(path, protected)?;
            }
        }
        Ok(())
    }

    /// Reject a path inside the site that crosses a symlink or a non-directory.
    pub(crate) fn validate_path_chain(&self, path: &Path) -> Result<(), OutputBoundaryError> {
        let relative = path
            .strip_prefix(&self.site_root)
            .map_err(|_| OutputBoundaryError::OutsideSite)?;
        let mut current = self.site_root.clone();

        for component in relative.components() {
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(metadata)
                    if super::automatic_path_is_link_like(&current, &metadata.file_type())
                        .map_err(|source| OutputBoundaryError::Io {
                            operation: "could not inspect the publication path",
                            source,
                        })? =>
                {
                    return Err(OutputBoundaryError::Symlink {
                        site_root: self.site_root.clone(),
                        path: current,
                    });
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(OutputBoundaryError::NonDirectory {
                        site_root: self.site_root.clone(),
                        path: current,
                    });
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(source) => {
                    return Err(OutputBoundaryError::Io {
                        operation: "could not read the output path",
                        source,
                    });
                }
            }
        }

        Ok(())
    }

    /// Reject `path` when it overlaps `protected`, comparing logical and physical spellings.
    ///
    /// A symlinked input reaches its target physically, so both spellings must be compared: the
    /// logical one the author declared and the physical one the filesystem resolves.
    pub(crate) fn validate_overlap(
        &self,
        path: &Path,
        protected: &ProtectedPath,
    ) -> Result<(), OutputBoundaryError> {
        if FilesystemSourceIdentity::from_path(path)
            .intersects(&FilesystemSourceIdentity::from_path(&protected.path))
        {
            return Err(OutputBoundaryError::ProtectedPath {
                site_root: self.site_root.clone(),
                kind: protected.kind,
                protected: protected.path.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OutputBoundaryError {
    #[error("{0}")]
    SitePath(#[from] super::SitePathError),

    #[error(
        "the site directory could not be used; run the command from the site directory or pass `--config`"
    )]
    InvalidSiteRoot,

    #[error(
        "`build.publish-dir` points outside the site directory; set `build.publish-dir` to a path below the site root"
    )]
    OutsideSite,

    #[error("the publication paths for `build.publish-dir` overlap the {kind} {}", protected_location(.site_root, .protected))]
    ProtectedPath {
        site_root: PathBuf,
        kind: &'static str,
        protected: PathBuf,
    },

    #[error(
        "publication path `{}` is a symbolic link; set `build.publish-dir` to a path whose output and workspace cross no symbolic links",
        display_path(.path, .site_root)
    )]
    Symlink { site_root: PathBuf, path: PathBuf },

    #[error(
        "publication path `{}` is not a directory; set `build.publish-dir` to another path",
        display_path(.path, .site_root)
    )]
    NonDirectory { site_root: PathBuf, path: PathBuf },

    #[error("{operation}: {}", path_failure_reason(.source))]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

/// Say where an input the site reads conflicts with the output.
fn protected_location(site_root: &Path, protected: &Path) -> String {
    match protected.strip_prefix(site_root) {
        Ok(relative) if relative.as_os_str().is_empty() => "at the site directory".to_string(),
        Ok(_) => format!("at `{}`", display_path(protected, site_root)),
        Err(_) => "outside the site directory".to_string(),
    }
}

/// The boundaries every declared hook output must respect.
///
/// A declaration is a promise about a path the author will write, so it is checked against the
/// site layout before anything runs and again against the observed site: the rule lives once,
/// and each stage renders its own failure from the same verdict.
pub(crate) struct DeclaredOutputBoundaries {
    site_root: PathBuf,
    site_identity: crate::filesystem::FilesystemSourceIdentity,
    reserved: [Option<ReservedPath>; 5],
    published: crate::filesystem::FilesystemSourceIdentity,
}

/// One path a site keeps for itself, with the noun a failure names it by.
struct ReservedPath {
    kind: &'static str,
    identity: crate::filesystem::FilesystemSourceIdentity,
}

/// Why a declared output is not a path this site can grant.
pub(crate) enum DeclaredOutputViolation {
    /// The declaration leaves the site root.
    OutsideSite,
    /// The declaration reaches a path the site keeps for itself: the configuration document
    /// that declared it, Tola's working storage, the site lock, or a recovery workspace
    /// beside a published or vendor path.
    InsideReserved { kind: &'static str, path: PathBuf },
    /// The declaration reaches the published output directory.
    InsidePublished,
    /// A declaration already granted to another entry intersects this one.
    OwnedElsewhere(usize),
}

impl DeclaredOutputBoundaries {
    /// Resolve the site boundaries one stage's declarations are checked against.
    ///
    /// `configuration` is the site's configuration document. `vendor_workspace` is the recovery
    /// directory beside `vendor.path`, absent when the site declares no vendor path.
    pub(crate) fn for_site(
        site_root: &Path,
        configuration: &Path,
        published_root: &Path,
        vendor_workspace: Option<&Path>,
    ) -> Self {
        // A hook may not rewrite the document that declares it: the run reads that exact
        // configuration, so a write would leave the file and the build disagreeing. The empty
        // spelling names no document, and would otherwise intersect every declaration.
        let configuration = (!configuration.as_os_str().is_empty())
            .then(|| ("the site configuration", configuration.to_path_buf()));
        let reserved = [
            configuration,
            Some(("the `.tola` directory", site_root.join(INTERNAL_DIR))),
            Some(("the site build lock", site_root.join(SITE_BUILD_LOCK_FILE))),
            publication_workspace(published_root).map(|path| ("the publication workspace", path)),
            vendor_workspace.map(|path| ("the vendor workspace", path.to_path_buf())),
        ]
        .map(|entry| {
            entry.map(|(kind, path)| ReservedPath {
                kind,
                identity: crate::filesystem::FilesystemSourceIdentity::from_path(&path),
            })
        });
        Self {
            site_root: site_root.to_path_buf(),
            site_identity: crate::filesystem::FilesystemSourceIdentity::from_path(site_root),
            reserved,
            published: crate::filesystem::FilesystemSourceIdentity::from_path(published_root),
        }
    }

    /// The first boundary `declared` crosses, checked against the entries granted before it.
    ///
    /// `granted` holds every declaration already accepted for this stage, in the order the
    /// author wrote them, so a shared path is reported against the entry that claimed it first.
    pub(crate) fn violation<'a>(
        &self,
        declared: &crate::filesystem::FilesystemSourceIdentity,
        granted: impl IntoIterator<Item = &'a crate::filesystem::FilesystemSourceIdentity>,
    ) -> Option<DeclaredOutputViolation> {
        if !declared.is_within(&self.site_identity) {
            return Some(DeclaredOutputViolation::OutsideSite);
        }
        if let Some(reserved) = self
            .reserved
            .iter()
            .flatten()
            .find(|reserved| declared.intersects(&reserved.identity))
        {
            return Some(DeclaredOutputViolation::InsideReserved {
                kind: reserved.kind,
                path: reserved.identity.logical_path().to_path_buf(),
            });
        }
        if declared.intersects(&self.published) {
            return Some(DeclaredOutputViolation::InsidePublished);
        }
        granted
            .into_iter()
            .enumerate()
            .find_map(|(index, existing)| existing.intersects(declared).then_some(index))
            .map(DeclaredOutputViolation::OwnedElsewhere)
    }

    /// Resolve one declared root-relative path against this site.
    pub(crate) fn resolve(&self, declared: &Path) -> crate::filesystem::FilesystemSourceIdentity {
        crate::filesystem::FilesystemSourceIdentity::from_path(&self.site_root.join(declared))
    }
}
