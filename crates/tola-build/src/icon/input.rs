//! Stable source reads and SVG directory membership retained by one build attempt.

use std::borrow::Cow;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancellation::BuildCancellation;
use crate::filesystem::open_regular_file;
use crate::filesystem::{FilesystemSourceIdentity, SourceOverrides};
use tola_typst::ContentDigest;

use super::IconError;

pub(super) const MAX_COLLECTION_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn ensure_collection_size(path: &Path, byte_len: u64) -> Result<(), IconError> {
    if byte_len > MAX_COLLECTION_BYTES as u64 {
        return Err(IconError::TooLarge {
            path: path.to_path_buf(),
            limit: MAX_COLLECTION_BYTES,
        });
    }
    Ok(())
}

fn physical_path_is_unchanged(source: &FilesystemSourceIdentity) -> bool {
    FilesystemSourceIdentity::from_path(source.logical_path()) == *source
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IconFileRead {
    pub(super) source: FilesystemSourceIdentity,
    digest: ContentDigest,
    pub(super) byte_len: usize,
    boundary: tola_typst::SourceBoundary,
}

impl IconFileRead {
    pub(super) fn is_fresh(
        &self,
        overrides: &SourceOverrides,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<bool> {
        cancellation.ensure_active()?;
        if self.boundary.check(self.source.logical_path()).is_err() {
            return Ok(false);
        }
        if !physical_path_is_unchanged(&self.source) {
            return Ok(false);
        }
        let fresh = match overrides.get_physical(self.source.physical_path()) {
            Some(source) => source.digest == self.digest,
            None => digest_source_file(self.source.logical_path(), cancellation)
                .is_ok_and(|digest| digest == self.digest),
        } && physical_path_is_unchanged(&self.source);
        cancellation.ensure_active()?;
        Ok(fresh)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SvgDirectoryRead {
    pub(super) source: FilesystemSourceIdentity,
    members: Arc<[PathBuf]>,
    boundary: tola_typst::SourceBoundary,
}

impl SvgDirectoryRead {
    pub(super) fn is_fresh(
        &self,
        overrides: &SourceOverrides,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<bool> {
        cancellation.ensure_active()?;
        if !physical_path_is_unchanged(&self.source) {
            return Ok(false);
        }
        let fresh = svg_paths(
            self.source.logical_path(),
            overrides,
            cancellation,
            &self.boundary,
        )
        .is_ok_and(|(members, _)| members.as_slice() == self.members.as_ref());
        cancellation.ensure_active()?;
        Ok(fresh)
    }
}

pub(super) fn read_file<'a>(
    path: &Path,
    overrides: &'a SourceOverrides,
    cancellation: &BuildCancellation,
    boundary: &tola_typst::SourceBoundary,
) -> anyhow::Result<(Cow<'a, [u8]>, IconFileRead)> {
    cancellation.ensure_active()?;
    boundary.check(path)?;
    let source = FilesystemSourceIdentity::from_path(path);
    if let Some(unsaved) = overrides.get_physical(source.physical_path()) {
        let byte_len = unsaved.text.len();
        ensure_collection_size(path, byte_len as u64)?;
        cancellation.ensure_active()?;
        if !physical_path_is_unchanged(&source) {
            return Err(IconError::Changed {
                path: path.to_path_buf(),
            }
            .into());
        }
        return Ok((
            Cow::Borrowed(unsaved.text.as_bytes()),
            IconFileRead {
                source,
                digest: unsaved.digest,
                byte_len,
                boundary: boundary.clone(),
            },
        ));
    }
    let mut file = open_regular_file(path, true).map_err(|source| IconError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::new();
    read_chunks(&mut file, path, cancellation, |chunk| {
        bytes.extend_from_slice(chunk);
    })?;
    let digest = ContentDigest::of(&bytes);
    drop(file);
    // Reopen the logical path: rewinding the first descriptor would keep reading an inode that
    // an editor or downloader has already replaced with a different file.
    boundary.check(path)?;
    let confirmed_digest = digest_source_file(path, cancellation)?;
    if confirmed_digest != digest || !physical_path_is_unchanged(&source) {
        return Err(IconError::Changed {
            path: path.to_path_buf(),
        }
        .into());
    }
    let byte_len = bytes.len();
    Ok((
        Cow::Owned(bytes),
        IconFileRead {
            source,
            digest,
            byte_len,
            boundary: boundary.clone(),
        },
    ))
}

fn digest_source_file(
    path: &Path,
    cancellation: &BuildCancellation,
) -> anyhow::Result<ContentDigest> {
    crate::filesystem::digest_file(path, Some(cancellation))
        .map(|digest| ContentDigest::from_bytes(*digest.as_bytes()))
        .map_err(|error| match error.downcast::<std::io::Error>() {
            Ok(source) => IconError::Read {
                path: path.to_path_buf(),
                source,
            }
            .into(),
            Err(error) => error,
        })
}

pub(super) fn read_bounded(
    reader: &mut File,
    path: &Path,
    cancellation: &BuildCancellation,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    read_chunks(reader, path, cancellation, |chunk| {
        bytes.extend_from_slice(chunk);
    })?;
    Ok(bytes)
}

fn read_chunks(
    reader: &mut File,
    path: &Path,
    cancellation: &BuildCancellation,
    mut consume: impl FnMut(&[u8]),
) -> anyhow::Result<()> {
    let length = reader
        .metadata()
        .map_err(|source| IconError::Read {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    ensure_collection_size(path, length)?;
    let mut consumed = 0u64;
    let mut overflow = None;
    crate::filesystem::read_chunks(reader, cancellation, |chunk| {
        consumed += chunk.len() as u64;
        if let Err(error) = ensure_collection_size(path, consumed) {
            overflow = Some(error);
            return std::ops::ControlFlow::Break(());
        }
        consume(chunk);
        std::ops::ControlFlow::Continue(())
    })
    .map(|_| ())
    .map_err(|error| match error {
        crate::filesystem::FileReadError::Io(source) => IconError::Read {
            path: path.to_path_buf(),
            source,
        }
        .into(),
        crate::filesystem::FileReadError::Cancelled(cancelled) => anyhow::Error::new(cancelled),
    })?;
    if let Some(error) = overflow {
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn svg_paths(
    root: &Path,
    overrides: &SourceOverrides,
    cancellation: &BuildCancellation,
    boundary: &tola_typst::SourceBoundary,
) -> anyhow::Result<(Vec<PathBuf>, SvgDirectoryRead)> {
    cancellation.ensure_active()?;
    boundary.check(root)?;
    let source = FilesystemSourceIdentity::from_path(root);
    let mut members = Vec::new();
    for (path, _) in overrides.sources() {
        cancellation.ensure_active()?;
        if let Ok(relative) = path.strip_prefix(source.physical_path())
            && is_svg_path(relative)
        {
            members.push(root.join(relative));
        }
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        cancellation.ensure_active()?;
        boundary.check(&directory)?;
        let directory_entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && directory == root
                    && !members.is_empty() =>
            {
                continue;
            }
            Err(source) => {
                return Err(IconError::Read {
                    path: directory,
                    source,
                }
                .into());
            }
        };
        let mut entries = Vec::new();
        for entry in directory_entries {
            cancellation.ensure_active()?;
            entries.push(entry.map_err(|source| IconError::Read {
                path: directory.clone(),
                source,
            })?);
        }
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            cancellation.ensure_active()?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| IconError::Read {
                path: path.clone(),
                source,
            })?;
            if crate::filesystem::automatic_path_is_link_like(&path, &file_type)? {
                return Err(IconError::LinkedDirectoryMember { path }.into());
            }
            if file_type.is_dir() {
                pending.push(path);
            } else if is_svg_path(&path) {
                members.push(path);
            }
        }
    }
    members.sort();
    members.dedup();
    let evidence = SvgDirectoryRead {
        source,
        members: members.clone().into(),
        boundary: boundary.clone(),
    };
    Ok((members, evidence))
}

fn is_svg_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

pub(super) fn icon_name(root: &Path, path: &Path) -> Result<String, IconError> {
    let relative = path
        .strip_prefix(root)
        .expect("SVG inventory paths belong to the declared root");
    let stem = relative.with_extension("");
    let components = stem
        .iter()
        .map(|component| component.to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| IconError::InvalidName {
            path: path.to_path_buf(),
        })?;
    let name = components.join("-");
    name.parse::<tola_icons::IconName>()
        .map_err(|_| IconError::InvalidName {
            path: path.to_path_buf(),
        })?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_source(path: &Path) -> anyhow::Result<(Cow<'static, [u8]>, IconFileRead)> {
        let overrides = SourceOverrides::default();
        let cancellation = BuildCancellation::new();
        let (bytes, read) = read_file(
            path,
            &overrides,
            &cancellation,
            &tola_typst::SourceBoundary::default(),
        )?;
        Ok((Cow::Owned(bytes.into_owned()), read))
    }

    #[test]
    fn unsaved_sources_replace_the_source_view() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("missing/icons");
        let path = root.join("nested/new.svg");
        let cancellation = BuildCancellation::new();
        let overrides =
            SourceOverrides::new(vec![(path.clone(), Arc::from("<svg/>"))], &cancellation).unwrap();
        let (members, membership) = svg_paths(
            &root,
            &overrides,
            &cancellation,
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap();
        assert_eq!(members, vec![path.clone()]);
        let (bytes, read) = read_file(
            &path,
            &overrides,
            &cancellation,
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap();
        assert_eq!(bytes.as_ref(), b"<svg/>");
        assert!(membership.is_fresh(&overrides, &cancellation).unwrap());
        assert!(read.is_fresh(&overrides, &cancellation).unwrap());
        assert!(
            !membership
                .is_fresh(&SourceOverrides::default(), &cancellation)
                .unwrap()
        );
        assert!(
            !read
                .is_fresh(&SourceOverrides::default(), &cancellation)
                .unwrap()
        );
        let changed = SourceOverrides::new(
            vec![(path, Arc::from("<svg viewBox='0 0 24 24'/>"))],
            &cancellation,
        )
        .unwrap();
        assert!(membership.is_fresh(&changed, &cancellation).unwrap());
        assert!(!read.is_fresh(&changed, &cancellation).unwrap());
        assert!(!root.exists());
    }

    #[test]
    fn cancelled_freshness_stays_cancelled() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unsaved.svg");
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let overrides =
            SourceOverrides::new(vec![(path.clone(), Arc::from("<svg/>"))], &cancellation).unwrap();
        let (_, read) = read_file(
            &path,
            &overrides,
            &cancellation,
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap();
        canceller.cancel();

        assert!(
            read.is_fresh(&overrides, &cancellation)
                .unwrap_err()
                .is::<crate::cancellation::BuildCancelled>()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsaved_members_follow_alias() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("icons");
        std::fs::create_dir(&root).unwrap();
        let alias = directory.path().join("alias");
        symlink(&root, &alias).unwrap();
        let cancellation = BuildCancellation::new();
        let text = Arc::<str>::from("<svg/>");
        let overrides = SourceOverrides::new(
            vec![
                (alias.join("nested/new.svg"), Arc::clone(&text)),
                (root.join("nested/new.svg"), text),
            ],
            &cancellation,
        )
        .unwrap();
        let (members, membership) = svg_paths(
            &alias,
            &overrides,
            &cancellation,
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap();
        assert_eq!(members, vec![alias.join("nested/new.svg")]);
        let (bytes, _) = read_file(
            &members[0],
            &overrides,
            &cancellation,
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap();
        assert_eq!(bytes.as_ref(), b"<svg/>");
        assert!(membership.is_fresh(&overrides, &cancellation).unwrap());
        assert!(!root.join("nested").exists());
    }

    #[test]
    fn evidence_stales_when_the_file_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.json");
        std::fs::write(&path, b"one").unwrap();
        let (_, evidence) = read_source(&path).unwrap();
        assert!(
            evidence
                .is_fresh(&SourceOverrides::default(), &BuildCancellation::new())
                .unwrap()
        );
        std::fs::write(&path, b"two").unwrap();
        assert!(
            !evidence
                .is_fresh(&SourceOverrides::default(), &BuildCancellation::new())
                .unwrap()
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            !evidence
                .is_fresh(&SourceOverrides::default(), &BuildCancellation::new())
                .unwrap()
        );
    }

    #[test]
    fn digest_covers_the_whole_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.json");
        let expected = (0..(48 * 1024 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(&path, &expected).unwrap();
        let cancellation = BuildCancellation::new();
        let overrides = SourceOverrides::default();
        let (bytes, evidence) = read_source(&path).unwrap();
        assert_eq!(bytes.as_ref(), expected);
        assert_eq!(
            digest_source_file(&path, &cancellation).unwrap(),
            evidence.digest
        );
        assert!(evidence.is_fresh(&overrides, &cancellation).unwrap());
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_COLLECTION_BYTES as u64 + 1)
            .unwrap();
        assert!(!evidence.is_fresh(&overrides, &cancellation).unwrap());
    }

    #[test]
    fn cancellation_stops_between_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.json");
        std::fs::write(&path, [0u8; 32 * 1024]).unwrap();
        let mut file = open_regular_file(&path, true).unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let mut consumed = 0;
        let error = read_chunks(&mut file, &path, &cancellation, |chunk| {
            consumed += chunk.len();
            canceller.cancel();
        })
        .unwrap_err();
        assert!(consumed > 0 && consumed < 32 * 1024);
        assert!(error.is::<crate::cancellation::BuildCancelled>());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_is_detected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.json");
        let replacement = directory.path().join("download.json");
        let first = vec![b'a'; 32 * 1024];
        let second = vec![b'b'; first.len()];
        std::fs::write(&path, &first).unwrap();
        std::fs::write(&replacement, &second).unwrap();
        let source = FilesystemSourceIdentity::from_path(&path);
        let cancellation = BuildCancellation::new();
        let mut file = open_regular_file(&path, true).unwrap();
        let mut replaced = false;
        let mut read_bytes = Vec::new();
        read_chunks(&mut file, &path, &cancellation, |chunk| {
            read_bytes.extend_from_slice(chunk);
            if !replaced {
                std::fs::rename(&replacement, &path).unwrap();
                replaced = true;
            }
        })
        .unwrap();
        assert_eq!(read_bytes, first);
        assert_eq!(
            digest_source_file(&path, &cancellation).unwrap(),
            ContentDigest::of(&second),
        );
        assert!(
            !IconFileRead {
                source,
                digest: ContentDigest::of(&first),
                byte_len: first.len(),
                boundary: tola_typst::SourceBoundary::default(),
            }
            .is_fresh(&SourceOverrides::default(), &cancellation)
            .unwrap()
        );
    }

    #[test]
    fn membership_detects_nested_renames() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("arrows")).unwrap();
        let path = directory.path().join("arrows/left.svg");
        std::fs::write(&path, b"<svg/>").unwrap();
        let (members, evidence) = svg_paths(
            directory.path(),
            &SourceOverrides::default(),
            &BuildCancellation::new(),
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap();
        assert_eq!(members.as_slice(), std::slice::from_ref(&path));
        assert_eq!(icon_name(directory.path(), &path).unwrap(), "arrows-left");
        assert!(
            evidence
                .is_fresh(&SourceOverrides::default(), &BuildCancellation::new())
                .unwrap()
        );
        std::fs::rename(&path, directory.path().join("arrows/right.svg")).unwrap();
        assert!(
            !evidence
                .is_fresh(&SourceOverrides::default(), &BuildCancellation::new())
                .unwrap()
        );
    }

    #[test]
    fn oversized_file_fails_before_allocation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.json");
        File::create(&path)
            .unwrap()
            .set_len(MAX_COLLECTION_BYTES as u64 + 1)
            .unwrap();
        let error = read_source(&path).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::TooLarge { .. })
        ));
    }

    #[test]
    fn cancelled_read_reports_cancellation() {
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();
        let error = read_file(
            Path::new("missing.svg"),
            &SourceOverrides::default(),
            &canceller.token(),
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::cancellation::BuildCancelled>(),
            Some(crate::cancellation::BuildCancelled)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_alias_invalidates_evidence() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.json");
        let second = directory.path().join("second.json");
        let alias = directory.path().join("collection.json");
        std::fs::write(&first, "identical bytes").unwrap();
        std::fs::write(&second, "identical bytes").unwrap();
        symlink(&first, &alias).unwrap();
        let (_, evidence) = read_source(&alias).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&second, &alias).unwrap();
        assert!(
            !evidence
                .is_fresh(&SourceOverrides::default(), &BuildCancellation::new())
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_subtrees_are_rejected() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("icons");
        std::fs::create_dir(&root).unwrap();
        symlink(&root, root.join("loop")).unwrap();
        let error = svg_paths(
            &root,
            &SourceOverrides::default(),
            &BuildCancellation::new(),
            &tola_typst::SourceBoundary::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::LinkedDirectoryMember { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn fifo_source_does_not_block() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let error = read_source(&path).unwrap_err();
        assert!(
            matches!(error.downcast_ref::<IconError>(), Some(IconError::Read { source, .. }) if source.kind() == std::io::ErrorKind::InvalidData)
        );
    }
}
