//! Logical and physical source identities and the watch boundaries they define.

use super::path::absolute_path;
use std::path::{Path, PathBuf};

/// Whether one observed source is an exact file or a recursively enumerated tree.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum FilesystemSourceKind {
    Tree,
    File,
}

/// A source's logical path and observed physical target.
///
/// Present sources retain their canonical target. A missing optional source
/// retains its normalized spelling and is never reported as canonical.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FilesystemSourceIdentity {
    logical: PathBuf,
    physical: FilesystemSourceLocated,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum FilesystemSourceLocated {
    Canonical(PathBuf),
    Normalized(PathBuf),
}

impl FilesystemSourceIdentity {
    pub fn from_path(path: &Path) -> Self {
        let logical = crate::filesystem::lexical_path_identity(path);
        match std::fs::canonicalize(path) {
            Ok(canonical) => Self::canonical(logical, canonical),
            Err(_) => Self::normalized(logical, crate::filesystem::normalize_existing_prefix(path)),
        }
    }

    pub(crate) fn canonical(logical: PathBuf, canonical: PathBuf) -> Self {
        Self {
            logical,
            physical: FilesystemSourceLocated::Canonical(canonical),
        }
    }

    pub(crate) fn normalized(logical: PathBuf, physical: PathBuf) -> Self {
        Self {
            logical,
            physical: FilesystemSourceLocated::Normalized(physical),
        }
    }

    pub fn logical_path(&self) -> &Path {
        &self.logical
    }

    pub fn canonical_path(&self) -> Option<&Path> {
        match &self.physical {
            FilesystemSourceLocated::Canonical(path) => Some(path),
            FilesystemSourceLocated::Normalized(_) => None,
        }
    }

    pub fn physical_path(&self) -> &Path {
        match &self.physical {
            FilesystemSourceLocated::Canonical(path)
            | FilesystemSourceLocated::Normalized(path) => path,
        }
    }

    pub fn changed_paths_intersect(&self, changed_paths: &[PathBuf]) -> bool {
        changed_paths.iter().any(|changed| {
            let changed = absolute_path(changed);
            paths_intersect(&changed, &self.logical)
                || paths_intersect(&changed, self.physical_path())
        })
    }

    /// Whether both logical and physical paths are within the corresponding boundary paths.
    pub fn is_within(&self, boundary: &Self) -> bool {
        path_is_within(self.logical_path(), boundary.logical_path())
            && path_is_within(self.physical_path(), boundary.physical_path())
    }

    /// Whether an event path intersects this source's logical or physical path.
    /// Ancestor events count because replacing an ancestor can change the source.
    pub fn intersects_path(&self, path: &Path) -> bool {
        let path = Self::from_path(path);
        self.intersects(&path)
    }

    /// Whether logical or physical ownership intersects under native and
    /// portable path identities.
    pub fn intersects(&self, other: &Self) -> bool {
        [self.logical_path(), self.physical_path()]
            .into_iter()
            .any(|left| {
                [other.logical_path(), other.physical_path()]
                    .into_iter()
                    .any(|right| {
                        paths_intersect(left, right)
                            || portable_path_identities_intersect(left, right)
                    })
            })
    }
}

/// A source's logical and physical paths with its file or recursive-tree watch scope.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FilesystemWatchBoundary {
    kind: FilesystemSourceKind,
    source: FilesystemSourceIdentity,
}

impl FilesystemWatchBoundary {
    pub(crate) fn kind(&self) -> FilesystemSourceKind {
        self.kind
    }

    pub(crate) fn logical_path(&self) -> &Path {
        self.source.logical_path()
    }

    pub(crate) fn physical_path(&self) -> &Path {
        self.source.physical_path()
    }
}

/// Exact file and recursive tree watch requirements from frozen observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemWatchEvidence {
    boundaries: Vec<FilesystemWatchBoundary>,
}

impl FilesystemWatchEvidence {
    pub(crate) fn from_sources(
        sources: impl IntoIterator<Item = (FilesystemSourceKind, FilesystemSourceIdentity)>,
    ) -> Self {
        let mut boundaries = sources
            .into_iter()
            .map(|(kind, source)| FilesystemWatchBoundary { kind, source })
            .collect::<Vec<_>>();
        boundaries.sort_unstable();
        boundaries.dedup();
        Self { boundaries }
    }

    pub(crate) fn boundaries(&self) -> &[FilesystemWatchBoundary] {
        &self.boundaries
    }
}

pub(crate) fn paths_intersect(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn portable_path_identities_intersect(left: &Path, right: &Path) -> bool {
    let Some(left) = portable_path_key(left) else {
        return false;
    };
    let Some(right) = portable_path_key(right) else {
        return false;
    };
    left.starts_with(&right) || right.starts_with(&left)
}

fn path_is_within(path: &Path, boundary: &Path) -> bool {
    path.starts_with(boundary)
        || portable_path_key(path)
            .zip(portable_path_key(boundary))
            .is_some_and(|(path, boundary)| path.starts_with(&boundary))
}

fn portable_path_key(path: &Path) -> Option<Vec<String>> {
    path.components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(tola_address::portable_collision_key)
        })
        .collect()
}
