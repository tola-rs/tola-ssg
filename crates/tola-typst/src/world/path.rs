//! Compiler source path normalization.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Normalize a compilation root or source identity, including sources not yet on disk.
///
/// Relative paths are joined to the current directory with [`Path::join`] when
/// available. `.` and `..` are removed **before** any filesystem lookup:
/// `alias/../source.typ` uses the lexical parent of `alias`, even if `alias` is
/// a symlink. Parents beyond the path's root are discarded.
///
/// The cleaned path is canonicalized if possible. Otherwise ancestors are
/// canonicalized from nearest to farthest and the remaining normal components
/// appended, so a missing source under a symlinked directory uses that
/// directory's physical identity. A removed `missing/..` component need not
/// exist.
///
/// # Failure and platform behavior
///
/// Best-effort identity normalization, not validation of existence, access, or
/// containment. Canonicalization errors, including permission errors, are not
/// returned. If no prefix can be canonicalized, the cleaned spelling is
/// returned. If the current directory is unavailable, cleanup starts from the
/// original relative spelling; unmatched leading `..` are discarded rather than
/// retained, and the result can remain relative. Windows drive-relative inputs
/// follow `Path::join`, not [`std::path::absolute`], rules.
///
/// An empty input names the current directory when available. Path components
/// stay native OS strings, without lossy Unicode conversion; successful
/// canonicalization may use Windows extended-length path syntax.
///
/// This compiler identity differs deliberately from
/// `tola_build::filesystem::lexical_path_identity`, which preserves relative
/// leading parents and never touches the filesystem, and from
/// `tola_build::filesystem::normalize_path` and `normalize_existing_prefix`,
/// which canonicalize the original spelling before any fallback.
#[inline]
pub fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let absolute = lexical_normalize(&absolute);

    if let Ok(path) = absolute.canonicalize() {
        return path;
    }

    let mut ancestor = absolute.as_path();
    let mut missing = Vec::<OsString>::new();
    while let Some(name) = ancestor.file_name() {
        missing.push(name.to_owned());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
        if let Ok(mut resolved) = ancestor.canonicalize() {
            for component in missing.iter().rev() {
                resolved.push(component);
            }
            return lexical_normalize(&resolved);
        }
    }

    absolute
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    use tempfile::TempDir;

    use super::normalize_path;

    #[cfg(unix)]
    #[test]
    fn missing_child_uses_symlinked_parent() {
        let directory = TempDir::new().unwrap();
        let real = directory.path().join("real");
        let alias = directory.path().join("alias");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        assert_eq!(
            normalize_path(&alias.join("missing/child.typ")),
            real.canonicalize().unwrap().join("missing/child.typ")
        );
    }

    #[cfg(unix)]
    #[test]
    fn parents_apply_before_symlink_resolution() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let target = root.join("target");
        fs::create_dir_all(target.join("nested")).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(target.join("nested"), &alias).unwrap();
        fs::write(root.join("page.typ"), "lexical source").unwrap();
        fs::write(target.join("page.typ"), "physical source").unwrap();
        let path = alias.join("../page.typ");

        assert_eq!(path.canonicalize().unwrap(), target.join("page.typ"));
        assert_eq!(normalize_path(&path), root.join("page.typ"));
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_drops_parents_keeping_names() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let directory = TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let name = OsStr::from_bytes(b"source-\xff.typ");
        let path = root.join("missing/..").join(name);

        assert_eq!(normalize_path(&path), root.join(name));
    }

    #[test]
    fn empty_path_uses_current_directory() {
        assert_eq!(
            normalize_path(Path::new("")),
            fs::canonicalize(".").unwrap()
        );
    }
}
