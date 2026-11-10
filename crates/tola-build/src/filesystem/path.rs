//! Site-relative path resolution and author-facing failure reasons.

use std::path::{Component, Path, PathBuf};

use tola_input::normalize_existing_prefix;

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

/// A path declared in the site could not be resolved against its site root.
#[derive(Debug, thiserror::Error)]
pub enum SitePathError {
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
