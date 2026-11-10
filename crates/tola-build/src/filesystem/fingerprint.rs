//! Fingerprints of filesystem paths, entry kinds, and contents.

use std::fs::{self, File};
use std::io;
use std::ops::ControlFlow;

use crate::cancellation::{BuildCancellation, OptionalCancellation};
use anyhow::Result;
use std::path::Path;
use tola_typst::hash_length_prefixed;

use super::identity::FilesystemSourceKind;
use crate::filesystem::FileReadErrorExt;
use tola_input::read_chunks;

/// A BLAKE3 fingerprint of filesystem entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PathFingerprint([u8; 32]);

/// A root's kind and path-sensitive fingerprint from one traversal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathContents {
    pub(crate) kind: Option<FilesystemSourceKind>,
    pub(crate) fingerprint: PathFingerprint,
}

/// Read the root and each entry without following symbolic links.
pub(crate) fn path_contents(
    path: &Path,
    cancellation: Option<&BuildCancellation>,
) -> Result<Option<PathContents>> {
    cancellation.ensure_active_if_present()?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut tree = blake3::Hasher::new();
    tree.update(b"tola-directory\0");
    let kind = hash_entries(path, path, metadata, &mut tree, cancellation)?;
    Ok(Some(PathContents {
        kind,
        fingerprint: PathFingerprint(*tree.finalize().as_bytes()),
    }))
}

fn hash_entries(
    root: &Path,
    path: &Path,
    metadata: fs::Metadata,
    tree: &mut blake3::Hasher,
    cancellation: Option<&BuildCancellation>,
) -> Result<Option<FilesystemSourceKind>> {
    cancellation.ensure_active_if_present()?;
    let relative = path
        .strip_prefix(root)
        .expect("walked entry must remain beneath its root");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tola-entry\0");
    let kind = hash_entry(path, relative, &metadata, &mut hasher, cancellation)?;
    tree.update(hasher.finalize().as_bytes());
    if kind == Some(FilesystemSourceKind::Tree) {
        let mut children = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        children.sort_by_cached_key(fs::DirEntry::file_name);
        for child in children {
            cancellation.ensure_active_if_present()?;
            let child_path = child.path();
            let metadata = fs::symlink_metadata(&child_path)?;
            let _ = hash_entries(root, &child_path, metadata, tree, cancellation)?;
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
            read_chunks(&mut file, cancellation.flag(), |chunk| {
                hasher.update(chunk);
                ControlFlow::Continue(())
            })
            .map(|_| ())
            .map_err(FileReadErrorExt::into_anyhow)?;
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
    fn fingerprint_tracks_membership_and_bytes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("output");

        assert!(path_contents(&root, None).unwrap().is_none());
        fs::create_dir_all(root.join("assets")).unwrap();
        let empty_tree = path_contents(&root, None).unwrap().unwrap();
        assert_eq!(empty_tree.kind, Some(FilesystemSourceKind::Tree));

        fs::write(root.join("assets/app.css"), "v1").unwrap();
        let with_file = path_contents(&root, None).unwrap().unwrap();
        assert_ne!(empty_tree, with_file);

        fs::write(root.join("assets/app.css"), "v2").unwrap();
        let changed_contents = path_contents(&root, None).unwrap().unwrap();
        assert_ne!(with_file, changed_contents);

        fs::rename(root.join("assets/app.css"), root.join("assets/site.css")).unwrap();
        assert_ne!(
            changed_contents,
            path_contents(&root, None).unwrap().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn fingerprint_tracks_link_targets() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("tree");
        fs::create_dir(&root).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink("first", &link).unwrap();
        let first = path_contents(&root, None).unwrap();
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("second", &link).unwrap();
        let second = path_contents(&root, None).unwrap();
        assert_ne!(first, second);
        fs::remove_file(&link).unwrap();
        fs::write(&link, b"second").unwrap();
        assert_ne!(second, path_contents(&root, None).unwrap());
    }
}
