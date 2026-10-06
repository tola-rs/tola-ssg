//! Immutable source bytes, digests, and directory membership.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::identity::{FilesystemSourceIdentity, paths_intersect};
use super::path::{absolute_path, display_path, path_failure_reason};
use super::sys::automatic_path_is_link_like;

/// Reader noun for a source file inside an observation.
const SOURCE_NOUN: &str = "source";
/// Reader noun for the root of a recursive source tree.
const SOURCE_ROOT_NOUN: &str = "source root";

/// One regular file frozen inside a filesystem source observation.
#[derive(Clone, Debug)]
pub(crate) struct FilesystemSourceFile {
    relative: PathBuf,
    canonical_target: PathBuf,
    digest: tola_typst::ContentDigest,
    bytes: Arc<[u8]>,
}

impl FilesystemSourceFile {
    pub(crate) fn relative_path(&self) -> &Path {
        &self.relative
    }

    pub(crate) fn canonical_target(&self) -> &Path {
        &self.canonical_target
    }

    pub(crate) fn digest(&self) -> tola_typst::ContentDigest {
        self.digest
    }

    pub(crate) fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }

    fn same_evidence_as(&self, other: &Self) -> bool {
        self.relative == other.relative
            && self.canonical_target == other.canonical_target
            && self.digest == other.digest
    }
}

/// Sorted membership, canonical targets, digests, and immutable bytes for one source.
#[derive(Clone, Debug)]
pub(crate) struct FilesystemSourceObservation {
    canonical_root: Option<PathBuf>,
    files: Arc<[FilesystemSourceFile]>,
    file_indices: Arc<HashMap<PathBuf, usize>>,
}

impl FilesystemSourceObservation {
    pub(crate) fn files(&self) -> &[FilesystemSourceFile] {
        &self.files
    }

    pub(crate) fn canonical_root(&self) -> Option<&Path> {
        self.canonical_root.as_deref()
    }

    pub(crate) fn same_evidence_as(&self, other: &Self) -> bool {
        self.canonical_root == other.canonical_root
            && self.files.len() == other.files.len()
            && self
                .files
                .iter()
                .zip(other.files.iter())
                .all(|(left, right)| left.same_evidence_as(right))
    }

    /// The same observation restricted to the files `keep` accepts.
    ///
    /// A caller that learns after observation which files of a source another
    /// declaration owns publishes exactly the retained membership.
    pub(crate) fn retaining(&self, keep: impl Fn(&FilesystemSourceFile) -> bool) -> Self {
        let Some(first_removed) = self.files.iter().position(|file| !keep(file)) else {
            return self.clone();
        };
        let mut files = self.files[..first_removed].to_vec();
        files.extend(
            self.files[first_removed + 1..]
                .iter()
                .filter(|file| keep(file))
                .cloned(),
        );
        observation_from_files(self.canonical_root.clone(), files)
    }
}

/// Observe one already-resolved recursive tree.
///
/// The caller must resolve the canonical root immediately before this call and
/// apply its root-symlink policy. Members that are symlinks or reparse points
/// are rejected. `display_root` is the site root that reported paths are relative to.
#[allow(clippy::too_many_arguments)]
pub(crate) fn observe_tree_source(
    source_identity: &FilesystemSourceIdentity,
    boundary: &tola_typst::SourceBoundary,
    previous: Option<&FilesystemSourceObservation>,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
    display_root: &Path,
    member_label: &str,
    validate_member: &dyn Fn(&Path, &Path) -> Result<()>,
) -> Result<FilesystemSourceObservation> {
    cancellation.ensure_active()?;
    boundary.check(source_identity.logical_path())?;
    let canonical_root = canonical_source_path(source_identity, display_root)?;
    if let Some(previous) = previous
        && !changed_paths.is_empty()
        && previous.canonical_root() == Some(canonical_root)
        && let Some(update) = refresh_tree_files(
            source_identity,
            boundary,
            previous,
            changed_paths,
            cancellation,
            display_root,
            validate_member,
        )?
    {
        return Ok(update);
    }

    let mut files = Vec::new();
    walk_tree_members(
        source_identity,
        cancellation,
        display_root,
        member_label,
        validate_member,
        &mut |path, relative| {
            files.push(read_source_file(
                path,
                relative.to_path_buf(),
                previous,
                display_root,
                boundary,
            )?);
            Ok(())
        },
    )?;
    files.sort_unstable_by(|left, right| left.relative.cmp(&right.relative));
    Ok(observation_from_files(
        Some(canonical_root.to_path_buf()),
        files,
    ))
}

/// Observe one already-resolved exact regular file.
///
/// `display_root` is the site root that reported paths are relative to.
pub(crate) fn observe_file_source(
    source_identity: &FilesystemSourceIdentity,
    boundary: &tola_typst::SourceBoundary,
    previous: Option<&FilesystemSourceObservation>,
    cancellation: &crate::cancellation::BuildCancellation,
    display_root: &Path,
) -> Result<FilesystemSourceObservation> {
    cancellation.ensure_active()?;
    boundary.check(source_identity.logical_path())?;
    let canonical_source = canonical_source_path(source_identity, display_root)?;
    let file = read_source_file(
        source_identity.logical_path(),
        PathBuf::new(),
        previous,
        display_root,
        boundary,
    )?;
    if file.canonical_target != canonical_source {
        return Err(source_repointed_error(
            source_identity.logical_path(),
            SOURCE_NOUN,
            display_root,
        ));
    }
    Ok(observation_from_files(
        Some(canonical_source.to_path_buf()),
        vec![file],
    ))
}

fn refresh_tree_files(
    source_identity: &FilesystemSourceIdentity,
    boundary: &tola_typst::SourceBoundary,
    previous: &FilesystemSourceObservation,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
    display_root: &Path,
    validate_member: &dyn Fn(&Path, &Path) -> Result<()>,
) -> Result<Option<FilesystemSourceObservation>> {
    let canonical_root = canonical_source_path(source_identity, display_root)?;
    let mut changed_indices = Vec::new();
    for changed in changed_paths {
        cancellation.ensure_active()?;
        let changed = absolute_path(changed);
        if !paths_intersect(&changed, source_identity.logical_path())
            && !paths_intersect(&changed, canonical_root)
        {
            continue;
        }
        let lookup = changed
            .strip_prefix(source_identity.logical_path())
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| changed.clone());
        let Some(index) = previous.file_indices.get(&lookup).copied() else {
            return Ok(None);
        };
        let expected = &previous.files[index];
        let logical_source = source_identity.logical_path().join(&expected.relative);
        let metadata = match std::fs::symlink_metadata(&logical_source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                anyhow::bail!(
                    "cannot inspect source `{}`: {}; check that the file is readable",
                    display_path(&logical_source, display_root),
                    path_failure_reason(&error),
                );
            }
        };
        if !metadata.is_file()
            || automatic_path_is_link_like(&logical_source, &metadata.file_type())?
        {
            return Ok(None);
        }
        validate_member(&expected.relative, &logical_source)?;
        changed_indices.push(index);
    }
    changed_indices.sort_unstable();
    changed_indices.dedup();
    if changed_indices.is_empty() {
        return Ok(None);
    }

    let mut files = previous.files.to_vec();
    for index in &changed_indices {
        cancellation.ensure_active()?;
        let expected = &previous.files[*index];
        files[*index] = read_source_file(
            &source_identity.logical_path().join(&expected.relative),
            expected.relative.clone(),
            Some(previous),
            display_root,
            boundary,
        )?;
        if files[*index].canonical_target != expected.canonical_target {
            // A parent may have been replaced by a symlink even when the event names only
            // the file. Rewalk to enforce the same membership rules as a cold observation.
            return Ok(None);
        }
    }
    ensure_root_target_unchanged(source_identity, display_root)?;
    // Membership, ordering, and physical targets are unchanged; all aliases keep their indices.
    Ok(Some(FilesystemSourceObservation {
        canonical_root: previous.canonical_root.clone(),
        files: files.into(),
        file_indices: Arc::clone(&previous.file_indices),
    }))
}

/// How one tree walk names and validates the members it reports.
struct TreeMemberWalk<'a> {
    display_root: &'a Path,
    member_label: &'a str,
    validate_member: &'a dyn Fn(&Path, &Path) -> Result<()>,
}

/// Walk every member of an already-resolved recursive tree without reading bytes.
///
/// Membership rules live here for every caller: a member that is a symbolic
/// link, a reparse point, or not a regular file is an error, and `validate_member`
/// applies the caller's own publication rules. `visit` receives each member's
/// path and its path relative to the tree root in a deterministic order. The
/// caller must resolve the canonical root immediately before this call and
/// apply its root-symlink policy.
pub(crate) fn walk_tree_members(
    source_identity: &FilesystemSourceIdentity,
    cancellation: &crate::cancellation::BuildCancellation,
    display_root: &Path,
    member_label: &str,
    validate_member: &dyn Fn(&Path, &Path) -> Result<()>,
    visit: &mut dyn FnMut(&Path, &Path) -> Result<()>,
) -> Result<()> {
    cancellation.ensure_active()?;
    let walk = TreeMemberWalk {
        display_root,
        member_label,
        validate_member,
    };
    scan_tree_directory(
        source_identity.logical_path(),
        source_identity.logical_path(),
        cancellation,
        &walk,
        visit,
    )?;
    ensure_root_target_unchanged(source_identity, walk.display_root)
}

fn scan_tree_directory(
    root: &Path,
    directory: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
    walk: &TreeMemberWalk<'_>,
    visit: &mut dyn FnMut(&Path, &Path) -> Result<()>,
) -> Result<()> {
    cancellation.ensure_active()?;
    let entries = crate::filesystem::read_sorted_entries(
        directory,
        cancellation,
        walk.display_root,
        "source",
    )?;
    for entry in entries {
        cancellation.ensure_active()?;
        let file_type = entry.file_type().map_err(|error| {
            anyhow::anyhow!(
                "cannot inspect source `{}`: {}",
                display_path(&entry.path(), walk.display_root),
                path_failure_reason(&error),
            )
        })?;
        let path = entry.path();
        if automatic_path_is_link_like(&path, &file_type)? {
            anyhow::bail!(
                "{} `{}` is a symbolic link or reparse point; replace it with a real file or directory",
                walk.member_label,
                display_path(&path, walk.display_root)
            );
        }
        let relative = path
            .strip_prefix(root)
            .expect("recursive filesystem source remains below its root");
        (walk.validate_member)(relative, &path)?;
        if file_type.is_dir() {
            scan_tree_directory(root, &path, cancellation, walk, visit)?;
        } else if file_type.is_file() {
            visit(&path, relative)?;
        } else {
            anyhow::bail!(
                "{} `{}` is not a regular file; move or delete it",
                walk.member_label,
                display_path(&path, walk.display_root)
            );
        }
    }
    Ok(())
}

fn read_source_file(
    logical_source: &Path,
    relative: PathBuf,
    previous: Option<&FilesystemSourceObservation>,
    display_root: &Path,
    boundary: &tola_typst::SourceBoundary,
) -> Result<FilesystemSourceFile> {
    let logical_source = absolute_path(logical_source);
    boundary.check(&logical_source)?;
    let canonical_target = canonical_source_target(&logical_source, SOURCE_NOUN, display_root)?;
    let source = std::fs::read(&logical_source).map_err(|error| {
        anyhow::anyhow!(
            "cannot read source `{}`: {}",
            display_path(&logical_source, display_root),
            path_failure_reason(&error),
        )
    })?;
    let target_after = canonical_source_target(&logical_source, SOURCE_NOUN, display_root)?;
    if target_after != canonical_target {
        return Err(source_repointed_error(
            &logical_source,
            SOURCE_NOUN,
            display_root,
        ));
    }
    let digest = tola_typst::ContentDigest::of(&source);
    let bytes = previous
        .and_then(|previous| {
            previous
                .files
                .binary_search_by(|entry| entry.relative.cmp(&relative))
                .ok()
                .map(|index| &previous.files[index])
        })
        .filter(|entry| entry.canonical_target == canonical_target && entry.digest == digest)
        .map(|entry| Arc::clone(&entry.bytes))
        .unwrap_or_else(|| Arc::from(source));
    Ok(FilesystemSourceFile {
        relative,
        canonical_target,
        digest,
        bytes,
    })
}

fn observation_from_files(
    canonical_root: Option<PathBuf>,
    files: Vec<FilesystemSourceFile>,
) -> FilesystemSourceObservation {
    let files = Arc::<[FilesystemSourceFile]>::from(files);
    let mut file_indices = HashMap::with_capacity(files.len().saturating_mul(2));
    for (index, file) in files.iter().enumerate() {
        file_indices.entry(file.relative.clone()).or_insert(index);
        file_indices
            .entry(file.canonical_target().to_path_buf())
            .or_insert(index);
    }
    FilesystemSourceObservation {
        canonical_root,
        files,
        file_indices: Arc::new(file_indices),
    }
}

fn ensure_root_target_unchanged(
    source_identity: &FilesystemSourceIdentity,
    display_root: &Path,
) -> Result<()> {
    let expected = canonical_source_path(source_identity, display_root)?;
    let logical_root = source_identity.logical_path();
    let canonical_after = canonical_source_target(logical_root, SOURCE_ROOT_NOUN, display_root)?;
    if canonical_after != expected {
        return Err(source_repointed_error(
            logical_root,
            SOURCE_ROOT_NOUN,
            display_root,
        ));
    }
    Ok(())
}

/// Canonicalize one source path that observation requires to still resolve.
fn canonical_source_target(
    logical_source: &Path,
    source_noun: &str,
    display_root: &Path,
) -> Result<PathBuf> {
    std::fs::canonicalize(logical_source).map_err(|error| {
        anyhow::anyhow!(
            "cannot find {source_noun} `{}`: {}",
            display_path(logical_source, display_root),
            path_failure_reason(&error),
        )
    })
}

/// The error for a source that was re-pointed while it was being read.
fn source_repointed_error(
    logical_source: &Path,
    source_noun: &str,
    display_root: &Path,
) -> anyhow::Error {
    anyhow::anyhow!(
        "{source_noun} `{}` changed while it was being read; rerun the build",
        display_path(logical_source, display_root)
    )
}

fn canonical_source_path<'a>(
    source_identity: &'a FilesystemSourceIdentity,
    display_root: &Path,
) -> Result<&'a Path> {
    source_identity.canonical_path().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot find source `{}`",
            display_path(source_identity.logical_path(), display_root)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(
        root: &Path,
        previous: Option<&FilesystemSourceObservation>,
        changed: &[PathBuf],
    ) -> FilesystemSourceObservation {
        observe_tree_source(
            &FilesystemSourceIdentity::from_path(root),
            &tola_typst::SourceBoundary::default(),
            previous,
            changed,
            &crate::cancellation::BuildCancellation::default(),
            root,
            "asset",
            &|_, _| Ok(()),
        )
        .unwrap()
    }

    #[test]
    fn duplicate_changes_reuse_index_and_match_full_scan() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let changed = root.join("a.txt");
        std::fs::write(&changed, "before").unwrap();
        std::fs::write(root.join("b.txt"), "untouched").unwrap();
        let before = observe(&root, None, &[]);
        std::fs::write(&changed, "after").unwrap();
        // Duplicate callbacks must still update one file while preserving the old snapshot.
        let after = observe(&root, Some(&before), &[changed.clone(), changed.clone()]);
        let full = observe(&root, None, &[]);
        assert!(after.same_evidence_as(&full));
        assert!(Arc::ptr_eq(&before.file_indices, &after.file_indices));
        assert!(Arc::ptr_eq(&before.files[1].bytes, &after.files[1].bytes));
        assert_eq!(before.files[0].bytes.as_ref(), b"before");
        assert_eq!(after.files[0].bytes.as_ref(), b"after");

        // Membership changes cannot reuse stale indices.
        std::fs::remove_file(&changed).unwrap();
        let removed = observe(&root, Some(&after), std::slice::from_ref(&changed));
        assert!(removed.same_evidence_as(&observe(&root, None, &[])));
        assert!(!Arc::ptr_eq(&after.file_indices, &removed.file_indices));
        std::fs::write(&changed, "restored").unwrap();
        let restored = observe(&root, Some(&removed), &[changed]);
        assert!(restored.same_evidence_as(&observe(&root, None, &[])));
    }

    #[test]
    fn unchanged_filter_reuses_files_and_reindexes_removals() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(root.join(name), name).unwrap();
        }
        let before = observe(root, None, &[]);
        let unchanged = before.retaining(|_| true);
        assert!(Arc::ptr_eq(&before.files, &unchanged.files));
        assert!(Arc::ptr_eq(&before.file_indices, &unchanged.file_indices));
        let filtered = before.retaining(|file| file.relative_path() != Path::new("b.txt"));
        assert_eq!(filtered.files.len(), 2);
        assert!(!filtered.file_indices.contains_key(Path::new("b.txt")));
        assert_eq!(filtered.file_indices[Path::new("c.txt")], 1);
        assert!(Arc::ptr_eq(
            &before.files[2].bytes,
            &filtered.files[1].bytes
        ));
        assert!(before.retaining(|_| false).files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn incremental_observation_rejects_replaced_parent_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("assets");
        let nested = root.join("nested");
        let moved = directory.path().join("moved");
        std::fs::create_dir_all(&nested).unwrap();
        let changed = nested.join("file.txt");
        std::fs::write(&changed, "before").unwrap();
        let before = observe(&root, None, &[]);
        std::fs::rename(&nested, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &nested).unwrap();
        let error = observe_tree_source(
            &FilesystemSourceIdentity::from_path(&root),
            &tola_typst::SourceBoundary::default(),
            Some(&before),
            &[changed],
            &crate::cancellation::BuildCancellation::default(),
            &root,
            "asset",
            &|_, _| Ok(()),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("symbolic link or reparse point"),
            "{error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn aliased_observation_accepts_physical_events() {
        let directory = tempfile::tempdir().unwrap();
        let physical = directory.path().join("physical");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&physical).unwrap();
        std::os::unix::fs::symlink(&physical, &alias).unwrap();
        let changed = physical.join("file.txt");
        std::fs::write(&changed, "before").unwrap();
        let before = observe(&alias, None, &[]);
        std::fs::write(&changed, "after").unwrap();
        let after = observe(&alias, Some(&before), &[changed.canonicalize().unwrap()]);
        assert!(after.same_evidence_as(&observe(&alias, None, &[])));
        assert!(Arc::ptr_eq(&before.file_indices, &after.file_indices));
    }
}
