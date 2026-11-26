//! Merge verified lock entries while holding the writer lock file.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use super::{InputLock, LockError};

impl InputLock {
    /// Commit changes relative to `orig`, preserving concurrent edits to other entries.
    pub fn commit(&self, site_root: &Path, orig: &Self) -> Result<bool, LockError> {
        if self == orig {
            return Ok(false);
        }
        let dir = crate::layout::state(site_root);
        std::fs::create_dir_all(&dir).map_err(LockError::Access)?;
        // Keep the pathname after closing so every writer locks the same file.
        let lock_file =
            tola_fs::open_lock_file(&dir.join("inputs.lock")).map_err(LockError::Access)?;
        lock_file.lock().map_err(LockError::Access)?;
        let cur = Self::read(site_root)?.unwrap_or_default();
        let mut merged = cur.clone();
        merge("modules", &orig.modules, &self.modules, &mut merged.modules)?;
        merge(
            "packages",
            &orig.packages,
            &self.packages,
            &mut merged.packages,
        )?;
        merge("icons", &orig.icons, &self.icons, &mut merged.icons)?;
        if merged == cur {
            return Ok(false);
        }
        merged.write_atomic(site_root)?;
        Ok(true)
    }

    fn write_atomic(&self, site_root: &Path) -> Result<(), LockError> {
        let text = self.to_toml()?;
        let path = Self::path(site_root);
        let mut tmp = tempfile::NamedTempFile::new_in(site_root)
            .map_err(|source| LockError::Stage { source })?;
        tmp.write_all(text.as_bytes())
            .map_err(|source| LockError::Write { source })?;
        tmp.as_file_mut()
            .sync_all()
            .map_err(|source| LockError::Flush { source })?;
        tmp.persist(&path).map_err(|err| LockError::Replace {
            path,
            source: err.error,
        })?;
        Ok(())
    }
}

fn merge<T: Clone + Eq>(
    section: &'static str,
    orig: &BTreeMap<String, T>,
    updated: &BTreeMap<String, T>,
    cur: &mut BTreeMap<String, T>,
) -> Result<(), LockError> {
    let names: BTreeSet<_> = orig.keys().chain(updated.keys()).collect();
    for name in names {
        let before = orig.get(name);
        let after = updated.get(name);
        if before == after || cur.get(name) == after {
            continue;
        }
        if cur.get(name) != before {
            return Err(LockError::Conflict {
                section,
                name: name.clone(),
            });
        }
        match after {
            Some(entry) => {
                cur.insert(name.clone(), entry.clone());
            }
            None => {
                cur.remove(name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LockedPackage, Sha256Digest};

    fn pkg(bytes: &[u8]) -> LockedPackage {
        LockedPackage {
            version: "1.0.0".into(),
            tree_sha256: Sha256Digest::of_bytes(bytes),
        }
    }

    #[test]
    fn commit_merge_and_conflict() {
        let site = tempfile::tempdir().unwrap();
        let orig = InputLock::default();
        let mut first = orig.clone();
        first
            .packages
            .insert("@preview/first".into(), pkg(b"first"));
        assert!(first.commit(site.path(), &orig).unwrap());
        let mut second = orig.clone();
        second
            .packages
            .insert("@preview/second".into(), pkg(b"second"));
        assert!(second.commit(site.path(), &orig).unwrap());
        assert!(!first.commit(site.path(), &orig).unwrap());
        let cur = InputLock::read(site.path()).unwrap().unwrap();
        assert_eq!(cur.packages.len(), 2);

        second
            .packages
            .insert("@preview/first".into(), pkg(b"changed"));
        assert!(matches!(
            second.commit(site.path(), &orig),
            Err(LockError::Conflict { .. })
        ));
        assert_eq!(InputLock::read(site.path()).unwrap().unwrap(), cur);

        let mut removed = cur.clone();
        removed.packages.remove("@preview/first");
        assert!(removed.commit(site.path(), &cur).unwrap());
        assert_eq!(
            InputLock::read(site.path()).unwrap().unwrap().packages,
            removed.packages
        );
    }
}
