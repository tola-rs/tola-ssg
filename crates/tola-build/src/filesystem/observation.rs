//! Immutable source bytes, digests, and directory membership.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::path::{absolute_path, path_failure_reason};
use tola_diagnostic::display_path;
use tola_fs::{entry_is_link_like, is_link_like};
use tola_input::FilesystemSourceIdentity;
use tola_input::paths_intersect;

/// Reader noun for a source file inside an observation.
const SOURCE_NOUN: &str = "source";
/// Reader noun for the root of a recursive source tree.
const SOURCE_ROOT_NOUN: &str = "source root";

/// One regular file frozen inside a filesystem source observation.
#[derive(Clone, Debug)]
pub(crate) struct FilesystemSourceFile {
    relative: PathBuf,
    canonical_target: PathBuf,
    followed_symlink: bool,
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
            && self.followed_symlink == other.followed_symlink
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

    pub(crate) fn linked_files(&self) -> impl Iterator<Item = &FilesystemSourceFile> {
        self.files.iter().filter(|file| file.followed_symlink)
    }

    pub(crate) fn link_targets_changed(&self, root: &Path, changed_paths: &[PathBuf]) -> bool {
        !changed_paths.is_empty()
            && self.linked_files().any(|file| {
                let target = file.canonical_target();
                changed_paths
                    .iter()
                    .any(|changed| paths_intersect(changed, target))
                    || !root
                        .join(file.relative_path())
                        .canonicalize()
                        .is_ok_and(|current| current == target)
            })
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
}

/// The source identity, boundary, previous state, changed paths, and member policy of one tree observation.
pub(crate) struct TreeObservation<'a> {
    pub(crate) source_identity: &'a FilesystemSourceIdentity,
    pub(crate) boundary: &'a tola_typst::SourceBoundary,
    pub(crate) previous: Option<&'a FilesystemSourceObservation>,
    pub(crate) changed_paths: &'a [PathBuf],
    pub(crate) cancellation: &'a crate::cancellation::BuildCancellation,
    pub(crate) display_root: &'a Path,
    pub(crate) member_label: &'a str,
    pub(crate) follow_file_symlinks: bool,
    pub(crate) validate_member: &'a dyn Fn(&Path, &Path, Option<&Path>) -> Result<()>,
}

/// Observe selected files from an already-resolved tree while retaining their root identity.
///
/// The caller must resolve the canonical root immediately before this call.
/// `display_root` is the site root that reported paths are relative to.
pub(crate) fn observe_tree_source_selected(
    observation: TreeObservation<'_>,
    selected: &dyn Fn(&Path, bool) -> bool,
) -> Result<FilesystemSourceObservation> {
    let TreeObservation {
        source_identity,
        boundary,
        previous,
        changed_paths,
        cancellation,
        display_root,
        member_label,
        follow_file_symlinks,
        validate_member,
    } = observation;
    cancellation.ensure_active()?;
    boundary.check(source_identity.logical_path())?;
    let canonical_root = canonical_source_path(source_identity, display_root)?;
    if !follow_file_symlinks
        && let Some(previous) = previous
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
    walk_tree_members_selected(
        source_identity,
        cancellation,
        display_root,
        member_label,
        follow_file_symlinks,
        selected,
        validate_member,
        &mut |path, relative, target| {
            files.push(read_source_file(
                path,
                relative.to_path_buf(),
                previous,
                display_root,
                boundary,
                target,
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

/// Walk the selected union without opening excluded directories or reading unselected files.
pub(crate) fn walk_tree_members_selected(
    source_identity: &FilesystemSourceIdentity,
    cancellation: &crate::cancellation::BuildCancellation,
    display_root: &Path,
    member_label: &str,
    follow_file_symlinks: bool,
    selected: &dyn Fn(&Path, bool) -> bool,
    validate_member: &dyn Fn(&Path, &Path, Option<&Path>) -> Result<()>,
    visitor: &mut dyn FnMut(&Path, &Path, Option<&Path>) -> Result<()>,
) -> Result<()> {
    cancellation.ensure_active()?;
    if selected(Path::new(""), true) {
        scan_tree_directory(
            source_identity.logical_path(),
            source_identity.logical_path(),
            cancellation,
            &TreeMemberWalk {
                display_root,
                member_label,
                follow_file_symlinks,
                validate_member,
            },
            selected,
            visitor,
        )?;
    }
    ensure_root_target_unchanged(source_identity, display_root)
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
        None,
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
    validate_member: &dyn Fn(&Path, &Path, Option<&Path>) -> Result<()>,
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
        if !metadata.is_file() || is_link_like(&metadata) {
            return Ok(None);
        }
        validate_member(&expected.relative, &logical_source, None)?;
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
            None,
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
    follow_file_symlinks: bool,
    validate_member: &'a dyn Fn(&Path, &Path, Option<&Path>) -> Result<()>,
}

/// One recursive walk over the membership rules every caller shares: a member that is a symbolic
/// link, a reparse point, or not a regular file is an error when selected. Only a regular file
/// reaches `visit`.
fn scan_tree_directory(
    root: &Path,
    directory: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
    walk: &TreeMemberWalk<'_>,
    selected: &dyn Fn(&Path, bool) -> bool,
    visit: &mut dyn FnMut(&Path, &Path, Option<&Path>) -> Result<()>,
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
        let relative = path
            .strip_prefix(root)
            .expect("walked source remains below its root");
        let is_link = entry_is_link_like(&entry, &file_type)?;
        if is_link {
            let selected_file = selected(relative, false);
            if (selected_file || selected(relative, true))
                && std::fs::metadata(&path).is_ok_and(|target| target.is_dir())
            {
                return Err(directory_link_error(&path, walk.display_root));
            }
            if !selected_file {
                continue;
            }
            if file_type.is_symlink() && walk.follow_file_symlinks {
                let target = regular_file_symlink_target(&path, walk.display_root)?;
                (walk.validate_member)(relative, &path, Some(&target))?;
                visit(&path, relative, Some(&target))?;
                continue;
            }
            let help = if file_type.is_symlink() && !walk.follow_file_symlinks {
                "Set `follow-file-symlinks = true` on this asset tree to follow links to regular files, or exclude this path"
            } else {
                "Replace it with a real file or directory, or exclude this path"
            };
            return Err(tree_link_error(
                format!(
                    "{} `{}` is a symbolic link or reparse point",
                    walk.member_label,
                    display_path(&path, walk.display_root)
                ),
                help,
            ));
        }
        if !selected(relative, file_type.is_dir()) {
            continue;
        }
        (walk.validate_member)(relative, &path, None)?;
        if file_type.is_dir() {
            scan_tree_directory(root, &path, cancellation, walk, selected, visit)?;
        } else if file_type.is_file() {
            visit(&path, relative, None)?;
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

pub(crate) fn regular_file_symlink_target(path: &Path, display_root: &Path) -> Result<PathBuf> {
    let shown = display_path(path, display_root);
    let unreadable = |error: std::io::Error| {
        tree_link_error(
            format!(
                "cannot follow asset link `{shown}`: {}",
                path_failure_reason(&error)
            ),
            "Check that the target exists and is readable and that the link does not form a cycle",
        )
    };
    let target = std::fs::canonicalize(path).map_err(unreadable)?;
    let metadata = std::fs::symlink_metadata(&target).map_err(unreadable)?;
    if metadata.is_dir() {
        return Err(directory_link_error(path, display_root));
    }
    if !metadata.is_file() || is_link_like(&metadata) {
        return Err(tree_link_error(
            format!("asset link `{shown}` must point to a regular file"),
            "Link to a regular file or exclude this path",
        ));
    }
    Ok(target)
}

fn directory_link_error(path: &Path, display_root: &Path) -> anyhow::Error {
    let shown = display_path(path, display_root);
    tree_link_error(
        format!(
            "asset link `{shown}` points to a directory. Tola does not follow directory links within an asset tree"
        ),
        "Declare the directory as a separate `assets.trees` source or exclude this path",
    )
}

pub(crate) fn tree_link_error(message: String, help: &str) -> anyhow::Error {
    let diagnostic = tola_diagnostic::Diagnostic::new(
        tola_diagnostic::codes::build::SITE,
        tola_diagnostic::Severity::Error,
        &message,
    )
    .with_help(help);
    tola_diagnostic::DiagnosticError::new(message, vec![diagnostic]).into()
}

fn read_source_file(
    logical_source: &Path,
    relative: PathBuf,
    previous: Option<&FilesystemSourceObservation>,
    display_root: &Path,
    boundary: &tola_typst::SourceBoundary,
    target: Option<&Path>,
) -> Result<FilesystemSourceFile> {
    let logical_source = absolute_path(logical_source);
    boundary.check(&logical_source)?;
    let canonical_target = canonical_source_target(&logical_source, SOURCE_NOUN, display_root)?;
    if target.is_some_and(|target| target != canonical_target) {
        return Err(source_repointed_error(
            &logical_source,
            SOURCE_NOUN,
            display_root,
        ));
    }
    let mut source = Vec::new();
    tola_fs::open_regular_file(&canonical_target, false)
        .and_then(|mut file| file.read_to_end(&mut source))
        .map_err(|error| {
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
        followed_symlink: target.is_some(),
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

    fn observe_tree_source(
        observation: TreeObservation<'_>,
    ) -> Result<FilesystemSourceObservation> {
        observe_tree_source_selected(observation, &|_, _| true)
    }

    fn observe(
        root: &Path,
        previous: Option<&FilesystemSourceObservation>,
        changed: &[PathBuf],
    ) -> FilesystemSourceObservation {
        observe_tree_source(TreeObservation {
            source_identity: &FilesystemSourceIdentity::from_path(root),
            boundary: &tola_typst::SourceBoundary::default(),
            previous,
            changed_paths: changed,
            cancellation: &crate::cancellation::BuildCancellation::default(),
            display_root: root,
            member_label: "asset",
            follow_file_symlinks: false,
            validate_member: &|_, _, _| Ok(()),
        })
        .unwrap()
    }

    #[test]
    fn duplicate_index_match_full_scan() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let changed = root.join("a.txt");
        std::fs::write(&changed, "before").unwrap();
        std::fs::write(root.join("b.txt"), "untouched").unwrap();
        let before = observe(&root, None, &[]);
        std::fs::write(&changed, "after").unwrap();
        // Duplicate callbacks must still update one file while preserving the previous observation.
        let after = observe(&root, Some(&before), &[changed.clone(), changed.clone()]);
        let full = observe(&root, None, &[]);
        assert!(after.same_evidence_as(&full));
        assert_eq!(before.files[0].bytes.as_ref(), b"before");
        assert_eq!(after.files[0].bytes.as_ref(), b"after");

        // Membership changes cannot reuse stale indices.
        std::fs::remove_file(&changed).unwrap();
        let removed = observe(&root, Some(&after), std::slice::from_ref(&changed));
        assert!(removed.same_evidence_as(&observe(&root, None, &[])));
        std::fs::write(&changed, "restored").unwrap();
        let restored = observe(&root, Some(&removed), &[changed]);
        assert!(restored.same_evidence_as(&observe(&root, None, &[])));
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
        let error = observe_tree_source(TreeObservation {
            source_identity: &FilesystemSourceIdentity::from_path(&root),
            boundary: &tola_typst::SourceBoundary::default(),
            previous: Some(&before),
            changed_paths: &[changed],
            cancellation: &crate::cancellation::BuildCancellation::default(),
            display_root: &root,
            member_label: "asset",
            follow_file_symlinks: false,
            validate_member: &|_, _, _| Ok(()),
        })
        .unwrap_err();
        assert!(error.to_string().contains("nested"), "{error:#}");
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
    }
}
