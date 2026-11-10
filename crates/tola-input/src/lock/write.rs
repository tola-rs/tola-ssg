//! Merge verified entries while holding the input lock's writer file.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use super::{InputLock, LockError};

impl InputLock {
    /// Commit changes relative to `original`, preserving concurrent changes to other entries.
    ///
    /// Writers hold a separate file only while rereading, merging, and replacing `tola.lock`.
    /// A conflicting edit leaves the complete file untouched. Returns whether the file changed.
    pub fn commit(&self, site_root: &Path, original: &Self) -> Result<bool, LockError> {
        if self == original {
            return Ok(false);
        }
        let directory = crate::layout::state(site_root);
        std::fs::create_dir_all(&directory).map_err(LockError::Access)?;
        // Keep the pathname after closing: every writer must lock the same file.
        let writer =
            tola_fs::open_lock_file(&directory.join("inputs.lock")).map_err(LockError::Access)?;
        writer.lock().map_err(LockError::Access)?;
        let current = Self::read(site_root)?.unwrap_or_default();
        let mut merged = current.clone();
        merge(
            "modules",
            &original.modules,
            &self.modules,
            &mut merged.modules,
        )?;
        merge(
            "packages",
            &original.packages,
            &self.packages,
            &mut merged.packages,
        )?;
        merge("icons", &original.icons, &self.icons, &mut merged.icons)?;
        if merged == current {
            return Ok(false);
        }
        merged.write_atomic(site_root)?;
        Ok(true)
    }

    fn write_atomic(&self, site_root: &Path) -> Result<(), LockError> {
        let text = self.to_toml()?;
        let path = Self::path(site_root);
        let mut temporary = tempfile::NamedTempFile::new_in(site_root)
            .map_err(|source| LockError::Stage { source })?;
        temporary
            .write_all(text.as_bytes())
            .map_err(|source| LockError::Write { source })?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|source| LockError::Flush { source })?;
        temporary
            .persist(&path)
            .map_err(|error| LockError::Replace {
                path,
                source: error.error,
            })?;
        Ok(())
    }
}

fn merge<T: Clone + Eq>(
    section: &'static str,
    original: &BTreeMap<String, T>,
    updated: &BTreeMap<String, T>,
    current: &mut BTreeMap<String, T>,
) -> Result<(), LockError> {
    let names: BTreeSet<_> = original.keys().chain(updated.keys()).collect();
    for name in names {
        let before = original.get(name);
        let after = updated.get(name);
        if before == after || current.get(name) == after {
            continue;
        }
        if current.get(name) != before {
            return Err(LockError::Conflict {
                section,
                name: name.clone(),
            });
        }
        match after {
            Some(entry) => {
                current.insert(name.clone(), entry.clone());
            }
            None => {
                current.remove(name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LockedPackage, Sha256Digest};

    fn package(bytes: &[u8]) -> LockedPackage {
        LockedPackage {
            version: "1.0.0".into(),
            tree_sha256: Sha256Digest::of_bytes(bytes),
        }
    }

    #[test]
    fn stale_writers_preserve_other_entries_and_refuse_conflicts() {
        let site = tempfile::tempdir().unwrap();
        let original = InputLock::default();
        let mut first = original.clone();
        first
            .packages
            .insert("@preview/first".into(), package(b"first"));
        assert!(first.commit(site.path(), &original).unwrap());
        let mut second = original.clone();
        second
            .packages
            .insert("@preview/second".into(), package(b"second"));
        assert!(second.commit(site.path(), &original).unwrap());
        assert!(!first.commit(site.path(), &original).unwrap());
        let current = InputLock::read(site.path()).unwrap().unwrap();
        assert_eq!(current.packages.len(), 2);

        second
            .packages
            .insert("@preview/first".into(), package(b"changed"));
        assert!(matches!(
            second.commit(site.path(), &original),
            Err(LockError::Conflict { .. })
        ));
        assert_eq!(InputLock::read(site.path()).unwrap().unwrap(), current);

        let mut removed = current.clone();
        removed.packages.remove("@preview/first");
        assert!(removed.commit(site.path(), &current).unwrap());
        assert_eq!(
            InputLock::read(site.path()).unwrap().unwrap().packages,
            removed.packages
        );
    }
}
