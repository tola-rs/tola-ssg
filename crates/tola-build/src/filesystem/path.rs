//! Lexical and physical filesystem path identities.

use std::path::{Component, Path, PathBuf};

/// Form a lexical identity without consulting the filesystem or current directory.
///
/// Removes `.` and cancels a normal component followed by `..`. Unmatched
/// leading parents in a relative path are retained; rooted paths never traverse
/// above their root. A path that cancels completely stays empty, not `.`.
/// Components remain native OS strings, without lossy Unicode conversion.
///
/// Use this for logical names that must remain equal before and after a file
/// exists. It does not make relative paths absolute or follow symlinks:
/// `alias/../page.typ` becomes `page.typ` even when `alias` is a symlink.
/// Use [`normalize_path`] for an existing physical target, or
/// [`normalize_existing_prefix`] to resolve a missing suffix through its parent.
/// None of these operations validates access or containment.
pub fn lexical_path_identity(path: &Path) -> PathBuf {
    let mut identity = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => identity.push(prefix.as_os_str()),
            std::path::Component::RootDir => identity.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(
                    identity.components().next_back(),
                    Some(std::path::Component::Normal(_))
                ) {
                    identity.pop();
                } else if !identity.has_root() {
                    identity.push(component.as_os_str());
                }
            }
            std::path::Component::Normal(name) => identity.push(name),
        }
    }

    identity
}

/// Canonicalize the original path, falling back to its absolute spelling.
///
/// Use this for a present physical target. No lexical cleanup precedes
/// [`Path::canonicalize`]: on Unix, `alias/..` follows a symlink before traversing
/// its parent. This differs from [`lexical_path_identity`] and the compiler's
/// [`tola_typst::normalize_path`], which cancel that component lexically.
///
/// # Failure and platform behavior
///
/// Any canonicalization error, not only a missing path, falls back to
/// [`std::path::absolute`]. That fallback does not follow symlinks, including in
/// an existing prefix: a missing child of a symlink keeps the alias spelling.
/// Use [`normalize_existing_prefix`] when that prefix should be resolved.
///
/// The fallback uses the standard library's platform rules: on Unix it retains
/// `..` (including above the root); on Windows ordinary paths use
/// `GetFullPathNameW` normalization, while verbatim paths are left as given.
/// Successful canonicalization may use Windows extended-length path syntax.
/// No conversion drops or replaces a character.
///
/// If absolute conversion also fails, for example because the input is empty
/// or the current directory is unavailable, the original spelling is returned
/// unchanged and can remain relative. Errors are not returned; the result is
/// not proof that the path exists, is accessible, or lies within a boundary.
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use tola_build::filesystem::normalize_path;
///
/// let directory = std::env::current_dir()?;
/// assert_eq!(normalize_path(Path::new(".")), directory.canonicalize()?);
/// # Ok::<(), std::io::Error>(())
/// ```
#[inline]
pub fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve a physical prefix for comparisons involving paths not yet on disk.
///
/// First canonicalizes the original spelling, with the same platform-dependent
/// symlink/`..` order as [`normalize_path`]. On failure, makes the path absolute
/// if possible, removes trailing normal components until a prefix
/// [`exists`](Path::exists), normalizes that prefix with [`normalize_path`], and
/// appends the removed names. Missing normal suffixes under a symlinked
/// directory therefore use its physical identity, unlike [`normalize_path`].
///
/// # Failure and platform behavior
///
/// This is best effort, not a guarantee that the nearest ancestor is resolved.
/// The search stops at an existing prefix even if it cannot be canonicalized,
/// or when no trailing filename can be removed. In particular, on Unix an
/// unresolved `missing/..` stops the search and can leave earlier symlinks
/// unresolved. No lexical cleanup precedes the filesystem lookups; use the
/// compiler's [`tola_typst::normalize_path`] for its lexical-first source
/// identity, not as an interchangeable physical-path operation.
///
/// Absolute conversion and prefix normalization inherit [`normalize_path`]'s
/// platform and current-directory failure behavior. An empty input stays empty;
/// failures can leave the result relative. Names are appended as native OS
/// strings without lossy Unicode conversion. Existence and canonicalization
/// errors are not returned or distinguished from missing paths, and an existing
/// prefix need not be a directory. The result does not validate access or
/// containment and need not identify a path that can be opened.
#[inline]
pub fn normalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }

    // Start absolute so missing relative paths reach an existing root, not an empty path.
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut missing = Vec::new();
    let mut current = absolute.as_path();
    while !current.exists() {
        let Some(name) = current.file_name() else {
            break;
        };
        missing.push(name.to_owned());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }

    let mut normalized = normalize_path(current);
    for name in missing.iter().rev() {
        normalized.push(name);
    }
    normalized
}

/// Resolve `path` against the current directory without filesystem or symlink access.
///
/// Falls back to `path` unchanged when the platform cannot form an absolute path.
pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Borrow `path` relative to `root` when `path` is inside it, otherwise `path` unchanged.
pub(crate) fn root_relative<'a>(path: &'a Path, root: &Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

/// Render `path` for a site author: relative to `root` when it lies inside, with `/` separators.
///
/// This is the one spelling every message a site author reads uses, so a failure reads the same
/// on every platform. A path outside the site is named by its last component — the author reads
/// which file it is, never where this machine keeps it. The site directory itself is spelled `.`.
pub fn display_path(path: &Path, root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(root) {
        return render_named(relative);
    }
    if path.is_absolute() && root.is_absolute() {
        // Two spellings of one absolute directory (a symlinked temporary or home directory, say)
        // still name the same site, so compare the normalized forms. Only the directory spelling
        // is resolved: the final component is the entry the author wrote, and resolving it would
        // report a symbolic link's target instead of the link.
        let normalized_path = normalize_directory_prefix(path);
        let normalized_root = normalize_existing_prefix(root);
        if let Ok(relative) = normalized_path.strip_prefix(&normalized_root) {
            return render_named(relative);
        }
        if normalized_path.is_absolute() {
            return render_relative(Path::new(
                normalized_path
                    .file_name()
                    .unwrap_or(normalized_path.as_os_str()),
            ));
        }
    }
    render_relative(path)
}

/// Render a path the site contains, naming the site directory itself as `.`.
fn render_named(relative: &Path) -> String {
    if relative.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        render_relative(relative)
    }
}

/// Canonicalize the directory spelling while keeping the final component as written.
fn normalize_directory_prefix(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => normalize_existing_prefix(parent).join(name),
        _ => normalize_existing_prefix(path),
    }
}

pub(crate) fn render_relative(relative: &Path) -> String {
    relative
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// One plain-language reason a filesystem operation failed, in site-author words.
///
/// The operating system's message names errno values, device names, and mount points; the
/// author needs only the consequence for their site.
pub(crate) fn path_failure_reason(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => "it no longer exists",
        ErrorKind::PermissionDenied => "permission was denied",
        ErrorKind::IsADirectory => "it is a directory",
        ErrorKind::NotADirectory => "a parent directory does not exist",
        ErrorKind::ReadOnlyFilesystem => "the filesystem is read-only",
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => "the disk is full",
        ErrorKind::AlreadyExists => "it already exists",
        ErrorKind::DirectoryNotEmpty => "the directory is not empty",
        ErrorKind::InvalidData => "its contents are not valid",
        _ => "the filesystem refused the operation",
    }
}

/// Whether `path` is `root` itself or lies below it.
///
/// Both paths must already be normalized; this compares components, not strings.
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Normalization must reach a fixed point: a normalized path never normalizes further.
    ///
    /// Every logical identity, cache key, and containment check compares normalized paths.
    #[test]
    fn normalize_path_is_idempotent() {
        let directory = tempfile::TempDir::new().unwrap();
        let existing = directory.path().join("page.typ");
        std::fs::write(&existing, "page").unwrap();
        let missing = directory.path().join("missing/nested");
        let cases = [
            PathBuf::from(""),
            PathBuf::from("."),
            PathBuf::from(".."),
            PathBuf::from("a/../b"),
            PathBuf::from("/a/../b"),
            PathBuf::from("/../.."),
            PathBuf::from("文档/x.y"),
            existing,
            missing,
        ];

        for path in cases {
            let once = normalize_path(&path);
            assert_eq!(normalize_path(&once), once, "{path:?}");
        }
    }

    #[test]
    fn lexical_identity_ignores_existence() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("nested/../page.typ");
        let before = lexical_path_identity(&path);

        std::fs::write(temp.path().join("page.typ"), "page").unwrap();
        let while_present = lexical_path_identity(&path);
        std::fs::remove_file(temp.path().join("page.typ")).unwrap();
        let after = lexical_path_identity(&path);

        assert_eq!(before, temp.path().join("page.typ"));
        assert_eq!(while_present, before);
        assert_eq!(after, before);
    }

    #[test]
    fn lexical_identity_is_idempotent() {
        let once = lexical_path_identity(Path::new("/site/content/./posts/../index.typ"));
        assert_eq!(lexical_path_identity(&once), once);
    }

    #[test]
    fn relative_parents_stop_at_the_root() {
        assert_eq!(
            lexical_path_identity(Path::new("../posts/../../page.typ")),
            Path::new("../../page.typ")
        );
        assert_eq!(
            lexical_path_identity(Path::new("/../../posts/../../page.typ")),
            Path::new("/page.typ")
        );
    }

    #[test]
    fn existing_prefix_keeps_missing_at_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let existing = dir.path().join("@tola");
        std::fs::create_dir_all(&existing).unwrap();
        let missing = existing.join("pages");

        assert_eq!(
            normalize_existing_prefix(&missing),
            existing.canonicalize().unwrap().join("pages")
        );
    }

    #[test]
    fn empty_physical_paths_stay_empty() {
        assert_eq!(normalize_path(Path::new("")), Path::new(""));
        assert_eq!(normalize_existing_prefix(Path::new("")), Path::new(""));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_resolution_precedes_traversal() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let target = root.join("target");
        std::fs::create_dir_all(target.join("nested")).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(target.join("nested"), &alias).unwrap();
        std::fs::write(root.join("page.typ"), "lexical source").unwrap();
        std::fs::write(target.join("page.typ"), "physical source").unwrap();
        let path = alias.join("../page.typ");

        assert_eq!(lexical_path_identity(&path), root.join("page.typ"));
        assert_eq!(normalize_path(&path), target.join("page.typ"));
        assert_eq!(normalize_existing_prefix(&path), target.join("page.typ"));
    }

    #[cfg(unix)]
    #[test]
    fn prefix_normalization_resolves_link() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let path = alias.join("@tola/page.typ");
        let physical = target.join("@tola/page.typ");

        assert_eq!(normalize_path(&path), path);
        assert_eq!(normalize_existing_prefix(&path), physical);

        std::fs::create_dir(target.join("@tola")).unwrap();
        std::fs::write(&physical, "source").unwrap();

        assert_eq!(normalize_path(&path), physical);
        assert_eq!(normalize_existing_prefix(&path), physical);
    }

    #[cfg(unix)]
    #[test]
    fn unresolved_parent_blocks_symlink() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let path = alias.join("missing/../page.typ");

        assert_eq!(normalize_path(&path), path);
        assert_eq!(normalize_existing_prefix(&path), path);
    }

    #[cfg(unix)]
    #[test]
    fn missing_native_names_stay_non_unicode() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root
            .join("missing")
            .join(OsStr::from_bytes(b"source-\xff.typ"));

        assert_eq!(lexical_path_identity(&path), path);
        assert_eq!(normalize_path(&path), path);
        assert_eq!(normalize_existing_prefix(&path), path);
    }
}

/// A path declared in the site could not be resolved against its site root.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SitePathError {
    #[error("the site directory could not be used")]
    InvalidSiteRoot,
    #[error("the path leaves the site directory")]
    OutsideSite,
}

/// Make `path` absolute and lexical, without consulting the filesystem.
///
/// `.` components are dropped and `..` cancels the component before it. A path that would leave
/// its root is an error rather than a silently different location.
pub(crate) fn lexical_absolute(base: &Path, path: &Path) -> Result<PathBuf, SitePathError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();

    for component in candidate.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(SitePathError::OutsideSite);
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    Ok(normalized)
}

/// The canonical site root and the absolute spelling the caller supplied for it.
///
/// Both are needed: aliases of the root must resolve to the canonical root while the suffix is
/// retained, so a symlink inside a path stays visible to whoever owns that path.
pub(crate) fn canonical_site_root(site_root: &Path) -> Result<(PathBuf, PathBuf), SitePathError> {
    let supplied = if site_root.is_absolute() {
        lexical_absolute(site_root, site_root)?
    } else {
        let base = std::env::current_dir().map_err(|_| SitePathError::InvalidSiteRoot)?;
        lexical_absolute(&base, site_root)?
    };
    let canonical = supplied
        .canonicalize()
        .map_err(|_| SitePathError::InvalidSiteRoot)?;
    Ok((supplied, canonical))
}

/// Resolve `path` against the canonical site root, keeping an alias of the root canonical.
pub(crate) fn site_relative_absolute(
    supplied_root: &Path,
    canonical_root: &Path,
    path: &Path,
) -> Result<PathBuf, SitePathError> {
    let path = lexical_absolute(supplied_root, path)?;
    match path.strip_prefix(supplied_root) {
        Ok(relative) => lexical_absolute(canonical_root, relative),
        Err(_) => {
            for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
                if normalize_existing_prefix(ancestor) == canonical_root {
                    let relative = path
                        .strip_prefix(ancestor)
                        .expect("ancestor belongs to path");
                    return lexical_absolute(canonical_root, relative);
                }
            }
            Ok(path)
        }
    }
}
