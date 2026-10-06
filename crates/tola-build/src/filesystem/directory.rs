//! Cancellable directory enumeration and ownership of temporary directories.

use std::fs;
use std::path::Path;

use crate::cancellation::{BuildCancellation, OptionalCancellation};
use anyhow::Result;

/// Read a directory deterministically, checking cancellation while collecting its entries.
/// Name keys are allocated once per entry, rather than once per sorting comparison.
pub(crate) fn read_sorted_entries(
    directory: &Path,
    cancellation: &BuildCancellation,
    display_root: &Path,
    label: &str,
) -> Result<Vec<fs::DirEntry>> {
    let failed = |error: std::io::Error| {
        anyhow::anyhow!(
            "cannot read {label} directory `{}`: {}",
            super::display_path(directory, display_root),
            super::path_failure_reason(&error),
        )
    };
    cancellation.ensure_active()?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(failed)? {
        cancellation.ensure_active()?;
        entries.push(entry.map_err(failed)?);
    }
    entries.sort_by_cached_key(fs::DirEntry::file_name);
    cancellation.ensure_active()?;
    Ok(entries)
}

#[derive(Debug)]
pub(crate) struct TemporaryDirectory {
    directory: tempfile::TempDir,
}

impl TemporaryDirectory {
    pub(crate) fn create(parent: &Path, prefix: &str) -> Result<Self> {
        // The parent is always the site's private scratch directory: an absolute host
        // path would mean nothing to the author, so report the failure without it.
        let failed = |error: &std::io::Error| {
            anyhow::anyhow!(
                "cannot prepare the build's working directory: {}; check that the site directory is writable",
                super::path::path_failure_reason(error)
            )
        };
        fs::create_dir_all(parent).map_err(|error| failed(&error))?;
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(parent)
            .map_err(|error| failed(&error))?;
        Ok(Self { directory })
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn make_read_only(&self, cancellation: Option<&BuildCancellation>) -> Result<()> {
        set_tree_readonly(self.path(), true, cancellation)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = set_tree_readonly(self.path(), false, None);
    }
}

fn set_tree_readonly(
    path: &Path,
    readonly: bool,
    cancellation: Option<&BuildCancellation>,
) -> Result<()> {
    cancellation.ensure_active_if_present()?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    let set_permissions = || {
        let mut permissions = metadata.permissions();
        super::sys::set_write_permission(&mut permissions, !readonly, metadata.is_dir());
        fs::set_permissions(path, permissions)
    };
    if !readonly {
        set_permissions()?;
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            set_tree_readonly(&entry?.path(), readonly, cancellation)?;
        }
    }
    if readonly {
        set_permissions()?;
    }
    Ok(())
}
