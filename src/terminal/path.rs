//! Paths rendered the way the reader named them.
//!
//! One rule serves every message a site author reads outside the build library: show a path
//! relative to its site root when it lies inside, then relative to the current directory, and
//! only then as it was given.
//! [`tola_build::filesystem::display_path`] owns the site-level rendering; this module
//! adds the invocation-relative fallback that only the application knows about, plus
//! [`display_path_toward_home`] for a path under the reader's home directory.

use std::path::Path;

/// Render `path` for a diagnostic: relative to the current directory when it lies inside,
/// otherwise unchanged, and always with forward slashes.
pub(crate) fn display_path(path: &Path) -> String {
    match std::env::current_dir() {
        Ok(current) if path.starts_with(&current) => {
            tola_build::filesystem::display_path(path, &current)
        }
        _ => display_path_as_given(path),
    }
}

/// Render `path` for a diagnostic about `root`: relative to `root` when it lies inside, then
/// like [`display_path`]. `root` itself renders as `.`.
pub(crate) fn display_path_within(path: &Path, root: &Path) -> String {
    if path.starts_with(root) {
        tola_build::filesystem::display_path(path, root)
    } else {
        display_path(path)
    }
}

/// Render `path` exactly as the caller spelled it, with forward slashes.
///
/// A caller that already knows the spelling its reader needs — a configuration file beside the
/// site, an editor's own settings path — names that spelling here instead of assembling the text
/// itself.
pub(crate) fn display_path_as_given(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// Render `path` shortened toward the reader's home directory.
///
/// The home directory itself renders as `~` and everything inside it as `~/…`; every other path
/// keeps the normalized absolute spelling.
pub(crate) fn display_path_toward_home(path: &Path) -> String {
    display_path_with_home(path, std::env::var_os("HOME").as_deref().map(Path::new))
}

/// Render `path` with `home`, the directory a reader knows as `~`, when one is known.
fn display_path_with_home(path: &Path, home: Option<&Path>) -> String {
    let normalized = tola_build::filesystem::normalize_path(path);
    if let Some(home) = home {
        let home = tola_build::filesystem::normalize_path(home);
        if let Ok(rest) = normalized.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".into();
            }
            return format!("~/{}", rest.display());
        }
    }
    normalized.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_paths_stay_relative() {
        let current = std::env::current_dir().unwrap();

        assert_eq!(
            display_path(&current.join("content/index.typ")),
            "content/index.typ"
        );
        assert_eq!(
            display_path(Path::new("content/index.typ")),
            "content/index.typ"
        );
        assert_eq!(display_path(&current), ".");
    }

    #[test]
    fn given_path_keeps_its_spelling() {
        assert_eq!(
            display_path_as_given(Path::new("site/tola.toml")),
            "site/tola.toml"
        );
        #[cfg(windows)]
        assert_eq!(
            display_path_as_given(Path::new(r"site\tola.toml")),
            "site/tola.toml"
        );
    }

    #[test]
    fn paths_stay_relative_to_their_root() {
        let root = Path::new("/srv/site");

        assert_eq!(
            display_path_within(&root.join(".tola/logs/session.jsonl"), root),
            ".tola/logs/session.jsonl"
        );
        assert_eq!(display_path_within(root, root), ".");
        assert_eq!(
            display_path_within(Path::new("/tmp/session.jsonl"), root),
            "/tmp/session.jsonl"
        );
    }

    #[test]
    fn paths_under_home_shorten_to_tilde() {
        let home = Path::new("/home/reader");

        assert_eq!(
            display_path_with_home(&home.join("site/index.typ"), Some(home)),
            format!("~/site{}index.typ", std::path::MAIN_SEPARATOR)
        );
    }

    #[test]
    fn home_itself_renders_as_tilde() {
        let home = Path::new("/home/reader");

        assert_eq!(display_path_with_home(home, Some(home)), "~");
    }

    #[test]
    fn paths_outside_home_stay_unchanged() {
        let home = Path::new("/home/reader");

        for outside in [
            Path::new("/srv/site/index.typ"),
            Path::new("/home/reader2/site"),
        ] {
            let fallback = tola_build::filesystem::normalize_path(outside)
                .display()
                .to_string();
            assert_eq!(display_path_with_home(outside, Some(home)), fallback);
            assert_eq!(display_path_with_home(outside, None), fallback);
        }
    }
}
