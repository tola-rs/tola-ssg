//! Lexical and physical filesystem path identities.

use std::path::{Component, Path, PathBuf};

/// Why an input path cannot be written relative to its declaring directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelativePathError {
    /// Path is empty.
    #[error("is empty")]
    Empty,
    /// Path starts at a Unix or Windows filesystem root.
    #[error("starts at a filesystem root")]
    Root,
    /// Path uses a Windows drive-relative spelling.
    #[error("starts with a drive letter")]
    Drive,
    /// Path contains a `..` segment.
    #[error("contains `..`")]
    Parent,
}

/// Reject empty paths, filesystem roots, drive prefixes, and `..` segments on any platform.
pub fn validate_relative_path(path: &Path) -> Result<(), RelativePathError> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.is_empty() {
        return Err(RelativePathError::Empty);
    }
    if matches!(bytes, [b'/' | b'\\', ..]) {
        return Err(RelativePathError::Root);
    }
    if let [drive, b':', rest @ ..] = bytes
        && drive.is_ascii_alphabetic()
    {
        return Err(if matches!(rest, [b'/' | b'\\', ..]) {
            RelativePathError::Root
        } else {
            RelativePathError::Drive
        });
    }
    if bytes
        .split(|&b| matches!(b, b'/' | b'\\'))
        .any(|part| part == b"..")
    {
        return Err(RelativePathError::Parent);
    }
    Ok(())
}

/// Build a lexical path identity without consulting the filesystem or current directory.
///
/// Removes `.`, cancels `normal/..`, keeps leading `..` on relative paths, and never climbs past root.
pub fn lexical_path_identity(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for comp in path.components() {
        match comp {
            Component::Prefix(pfx) => out.push(pfx.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(name) => out.push(name),
        }
    }

    out
}

/// Canonicalize `path`, falling back to `std::path::absolute` and then `path` unchanged.
///
/// ```
/// use std::path::Path;
/// use tola_input::normalize_path;
///
/// let dir = std::env::current_dir()?;
/// assert_eq!(normalize_path(Path::new(".")), dir.canonicalize()?);
/// # Ok::<(), std::io::Error>(())
/// ```
#[inline]
pub fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Normalize the deepest existing prefix of `path` and re-append any missing trailing names.
#[inline]
pub fn normalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    // Start absolute so missing relative paths reach an existing root rather than empty path.
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut missing = Vec::new();
    let mut cur = abs.as_path();
    while !cur.exists() {
        let Some(name) = cur.file_name() else {
            break;
        };
        missing.push(name.to_owned());
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent;
    }

    let mut out = normalize_path(cur);
    for name in missing.iter().rev() {
        out.push(name);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let existing = dir.path().join("page.typ");
        std::fs::write(&existing, "page").unwrap();
        let missing = dir.path().join("missing/nested");
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
    fn lexical_ignores_existence() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested/../page.typ");
        let before = lexical_path_identity(&path);

        std::fs::write(dir.path().join("page.typ"), "page").unwrap();
        let present = lexical_path_identity(&path);
        std::fs::remove_file(dir.path().join("page.typ")).unwrap();
        let after = lexical_path_identity(&path);

        assert_eq!(before, dir.path().join("page.typ"));
        assert_eq!(present, before);
        assert_eq!(after, before);
    }

    #[test]
    fn relative_parents_stop() {
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
    fn existing_prefix_missing_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let existing = dir.path().join("@tola");
        std::fs::create_dir_all(&existing).unwrap();
        let missing = existing.join("pages");

        assert_eq!(
            normalize_existing_prefix(&missing),
            existing.canonicalize().unwrap().join("pages")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_before_parent() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
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
    fn prefix_resolves_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
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
    fn unresolved_parent_stops() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
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
    fn missing_non_unicode_name() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root
            .join("missing")
            .join(OsStr::from_bytes(b"source-\xff.typ"));

        assert_eq!(lexical_path_identity(&path), path);
        assert_eq!(normalize_path(&path), path);
        assert_eq!(normalize_existing_prefix(&path), path);
    }
}
