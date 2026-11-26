//! Logical and physical path identity for filesystem sources.

use std::path::{Path, PathBuf};

/// Logical path and observed physical target of a filesystem source.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FilesystemSourceIdentity {
    logical: PathBuf,
    physical: PhysicalPath,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PhysicalPath {
    Canonical(PathBuf),
    Normalized(PathBuf),
}

impl FilesystemSourceIdentity {
    pub fn from_path(path: &Path) -> Self {
        let logical = crate::lexical_path_identity(path);
        match std::fs::canonicalize(path) {
            Ok(canonical) => Self::canonical(logical, canonical),
            Err(_) => Self::normalized(logical, crate::normalize_existing_prefix(path)),
        }
    }

    pub fn canonical(logical: PathBuf, canonical: PathBuf) -> Self {
        Self {
            logical,
            physical: PhysicalPath::Canonical(canonical),
        }
    }

    pub fn normalized(logical: PathBuf, physical: PathBuf) -> Self {
        Self {
            logical,
            physical: PhysicalPath::Normalized(physical),
        }
    }

    pub fn logical_path(&self) -> &Path {
        &self.logical
    }

    pub fn canonical_path(&self) -> Option<&Path> {
        match &self.physical {
            PhysicalPath::Canonical(path) => Some(path),
            PhysicalPath::Normalized(_) => None,
        }
    }

    pub fn physical_path(&self) -> &Path {
        match &self.physical {
            PhysicalPath::Canonical(path) | PhysicalPath::Normalized(path) => path,
        }
    }

    pub fn changed_paths_intersect(&self, changed: &[PathBuf]) -> bool {
        changed.iter().any(|p| self.intersects(&Self::from_path(p)))
    }

    /// Return whether both logical and physical paths sit within `boundary`.
    pub fn is_within(&self, boundary: &Self) -> bool {
        path_is_within(self.logical_path(), boundary.logical_path())
            && path_is_within(self.physical_path(), boundary.physical_path())
    }

    /// Return whether `path` intersects this source's logical or physical path (including ancestors).
    pub fn intersects_path(&self, path: &Path) -> bool {
        self.intersects(&Self::from_path(path))
    }

    /// Return whether logical or physical paths intersect under native or portable identities.
    pub fn intersects(&self, other: &Self) -> bool {
        let lhs = [self.logical_path(), self.physical_path()];
        let rhs = [other.logical_path(), other.physical_path()];
        if lhs
            .into_iter()
            .any(|l| rhs.into_iter().any(|r| paths_intersect(l, r)))
        {
            return true;
        }
        let lhs_keys = lhs.map(portable_path_key);
        let rhs_keys = rhs.map(portable_path_key);
        lhs_keys.iter().flatten().any(|l| {
            rhs_keys
                .iter()
                .flatten()
                .any(|r| l.starts_with(r) || r.starts_with(l))
        })
    }
}

pub fn paths_intersect(lhs: &Path, rhs: &Path) -> bool {
    lhs == rhs || lhs.starts_with(rhs) || rhs.starts_with(lhs)
}

fn path_is_within(path: &Path, boundary: &Path) -> bool {
    path.starts_with(boundary)
        || portable_path_key(path)
            .zip(portable_path_key(boundary))
            .is_some_and(|(p, b)| p.starts_with(&b))
}

fn portable_path_key(path: &Path) -> Option<Vec<String>> {
    path.components()
        .map(|comp| {
            comp.as_os_str()
                .to_str()
                .map(tola_address::portable_collision_key)
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn alias_intersects_physical() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let alias = dir.path().join("alias");
        fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();
        fs::write(real.join("member.js"), "before").unwrap();
        let id = FilesystemSourceIdentity::from_path(&real);
        fs::remove_file(alias.join("member.js")).unwrap();
        assert!(id.changed_paths_intersect(&[alias.join("member.js")]));
    }
}
