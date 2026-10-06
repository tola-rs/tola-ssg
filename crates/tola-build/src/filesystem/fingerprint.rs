//! Fingerprints of filesystem paths, entry kinds, and contents.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::ops::ControlFlow;

use crate::cancellation::{BuildCancellation, OptionalCancellation};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tola_typst::hash_length_prefixed;

use super::identity::FilesystemSourceKind;
use super::read::{FileReadError, read_chunks};

/// A BLAKE3 fingerprint of filesystem entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PathFingerprint([u8; 32]);

impl PathFingerprint {
    #[inline]
    const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Compute a path- and type-sensitive fingerprint without following symlinks.
pub(crate) fn path_fingerprint(
    path: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<PathFingerprint> {
    let mut hasher = tree_hasher();
    let _ = visit_entries(path, cancellation, &mut |_, fingerprint| {
        hasher.update(&fingerprint.0);
    })?;
    Ok(PathFingerprint::new(*hasher.finalize().as_bytes()))
}

/// Aggregate an existing snapshot without reading the filesystem again.
pub(crate) fn snapshot_fingerprint(
    entries: &BTreeMap<PathBuf, PathFingerprint>,
) -> PathFingerprint {
    let mut hasher = tree_hasher();
    for fingerprint in entries.values() {
        hasher.update(&fingerprint.0);
    }
    PathFingerprint::new(*hasher.finalize().as_bytes())
}

fn tree_hasher() -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    // Fixed-width entry digests delimit entries independently of stat sizes.
    // The root entry distinguishes an empty directory from a missing path.
    hasher.update(b"tola-directory\0");
    hasher
}

/// The root kind and its contents come from the same filesystem traversal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathSnapshot {
    pub(crate) kind: Option<FilesystemSourceKind>,
    pub(crate) entries: BTreeMap<PathBuf, PathFingerprint>,
}

/// Snapshot a file or directory by relative path, including its root entry.
/// Directory entries record their kind; each file's bytes are read once.
/// Missing paths produce an empty snapshot.
pub(crate) fn entry_snapshot(
    path: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<PathSnapshot> {
    let mut entries = BTreeMap::new();
    let kind = visit_entries(path, cancellation, &mut |relative, fingerprint| {
        entries.insert(relative.to_path_buf(), fingerprint);
    })?;
    Ok(PathSnapshot { kind, entries })
}

fn visit_entries(
    path: &Path,
    cancellation: Option<&BuildCancellation>,
    visitor: &mut impl FnMut(&Path, PathFingerprint),
) -> Result<Option<FilesystemSourceKind>> {
    cancellation.ensure_active_if_present()?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => collect_entries(path, path, metadata, visitor, cancellation),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn collect_entries(
    root: &Path,
    path: &Path,
    metadata: fs::Metadata,
    visitor: &mut impl FnMut(&Path, PathFingerprint),
    cancellation: Option<&BuildCancellation>,
) -> Result<Option<FilesystemSourceKind>> {
    cancellation.ensure_active_if_present()?;
    let relative = path
        .strip_prefix(root)
        .expect("walked entry must remain beneath its root");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tola-entry\0");
    let kind = hash_entry(path, relative, &metadata, &mut hasher, cancellation)?;
    visitor(
        relative,
        PathFingerprint::new(*hasher.finalize().as_bytes()),
    );
    if kind == Some(FilesystemSourceKind::Tree) {
        let mut children = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        children.sort_unstable_by_key(|entry| entry.file_name());
        for child in children {
            cancellation.ensure_active_if_present()?;
            let child_path = child.path();
            let metadata = fs::symlink_metadata(&child_path)?;
            let _ = collect_entries(root, &child_path, metadata, visitor, cancellation)?;
        }
    }
    Ok(kind)
}

fn hash_entry(
    path: &Path,
    relative: &Path,
    metadata: &fs::Metadata,
    hasher: &mut blake3::Hasher,
    cancellation: Option<&BuildCancellation>,
) -> Result<Option<FilesystemSourceKind>> {
    cancellation.ensure_active_if_present()?;
    hash_length_prefixed(hasher, relative.as_os_str().as_encoded_bytes());
    if metadata.file_type().is_symlink() {
        hasher.update(b"symlink");
        hash_length_prefixed(hasher, fs::read_link(path)?.as_os_str().as_encoded_bytes());
    } else if metadata.is_dir() {
        hasher.update(b"directory");
        return Ok(Some(FilesystemSourceKind::Tree));
    } else if metadata.is_file() {
        hasher.update(b"file");
        hasher.update(digest_file(path, cancellation)?.as_bytes());
        return Ok(Some(FilesystemSourceKind::File));
    } else {
        hasher.update(b"other");
    }
    Ok(None)
}

/// Digest one local file's bytes, checking cancellation between read chunks.
///
/// `None` reads without observing cancellation because no attempt owns the file.
pub(crate) fn digest_file(
    path: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<blake3::Hash> {
    cancellation.ensure_active_if_present()?;
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    match cancellation {
        Some(cancellation) => {
            read_chunks(&mut file, cancellation, |chunk| {
                hasher.update(chunk);
                ControlFlow::Continue(())
            })
            .map(|_| ())
            .map_err(FileReadError::into_anyhow)?;
        }
        None => {
            std::io::copy(&mut file, &mut hasher)?;
        }
    }
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn path_fingerprint_tracks_contents() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("output");

        let missing = path_fingerprint(&root, None).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        let empty_tree = path_fingerprint(&root, None).unwrap();
        assert_ne!(missing, empty_tree);

        fs::write(root.join("assets/app.css"), "v1").unwrap();
        let with_file = path_fingerprint(&root, None).unwrap();
        assert_ne!(empty_tree, with_file);

        fs::write(root.join("assets/app.css"), "v2").unwrap();
        let changed_contents = path_fingerprint(&root, None).unwrap();
        assert_ne!(with_file, changed_contents);

        fs::rename(root.join("assets/app.css"), root.join("assets/site.css")).unwrap();
        assert_ne!(changed_contents, path_fingerprint(&root, None).unwrap());
    }

    #[test]
    fn snapshot_matches_streaming_fingerprint() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("tree");
        for phase in 0..3 {
            if phase == 1 {
                fs::create_dir(&root).unwrap();
            } else if phase == 2 {
                fs::create_dir(root.join("a")).unwrap();
                fs::write(root.join("a/z"), b"nested").unwrap();
                fs::write(root.join("a.txt"), b"sibling").unwrap();
            }
            assert_eq!(
                path_fingerprint(&root, None).unwrap(),
                snapshot_fingerprint(&entry_snapshot(&root, None).unwrap().entries)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn fingerprint_tracks_link_targets() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("tree");
        fs::create_dir(&root).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink("first", &link).unwrap();
        let first = path_fingerprint(&root, None).unwrap();
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("second", &link).unwrap();
        let second = path_fingerprint(&root, None).unwrap();
        assert_ne!(first, second);
        fs::remove_file(&link).unwrap();
        fs::write(&link, b"second").unwrap();
        assert_ne!(second, path_fingerprint(&root, None).unwrap());
    }

    #[test]
    fn snapshot_tracks_nested_paths() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("sources");
        fs::create_dir_all(root.join("generated")).unwrap();
        fs::write(root.join("generated/site.css"), "v1").unwrap();

        let first = entry_snapshot(&root, None).unwrap();
        fs::write(root.join("generated/site.css"), "v2").unwrap();
        fs::write(root.join("generated/extra.css"), "extra").unwrap();
        let second = entry_snapshot(&root, None).unwrap();

        assert_ne!(
            first.entries.get(Path::new("generated/site.css")),
            second.entries.get(Path::new("generated/site.css"))
        );
        assert!(
            second
                .entries
                .contains_key(Path::new("generated/extra.css"))
        );
        fs::remove_file(root.join("generated/site.css")).unwrap();
        let third = entry_snapshot(&root, None).unwrap();
        assert!(!third.entries.contains_key(Path::new("generated/site.css")));
    }

    #[test]
    fn snapshot_distinguishes_empty_directory() {
        let directory = TempDir::new().unwrap();
        let output = directory.path().join("generated");
        let missing = entry_snapshot(&output, None).unwrap();
        assert_eq!(missing.kind, None);
        assert!(missing.entries.is_empty());

        fs::create_dir(&output).unwrap();
        let tree = entry_snapshot(&output, None).unwrap();
        assert_eq!(tree.kind, Some(FilesystemSourceKind::Tree));
        assert!(tree.entries.contains_key(Path::new("")));

        fs::remove_dir(&output).unwrap();
        fs::write(&output, "generated").unwrap();
        let file = entry_snapshot(&output, None).unwrap();
        assert_eq!(file.kind, Some(FilesystemSourceKind::File));
        assert_ne!(tree, file);
    }
}
