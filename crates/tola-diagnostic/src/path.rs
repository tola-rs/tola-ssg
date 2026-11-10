//! Author-facing rendering of filesystem paths.

use std::path::{Path, PathBuf};

use tola_input::normalize_existing_prefix;

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

/// Render `relative` with `/` separators, whatever the host separator is.
pub fn render_relative(relative: &Path) -> String {
    relative
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}
