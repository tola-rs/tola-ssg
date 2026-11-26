//! Author-facing rendering of filesystem paths.

use std::path::{Path, PathBuf};

use tola_input::normalize_existing_prefix;

/// Render `path` relative to `root` with `/` separators; outside paths use their file name, and `root` itself is `.`.
pub fn display_path(path: &Path, root: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(root) {
        return render_named(rel);
    }
    if path.is_absolute() && root.is_absolute() {
        // Resolve directory symlinks only; keep the final component as written.
        let norm_path = normalize_directory_prefix(path);
        let norm_root = normalize_existing_prefix(root);
        if let Ok(rel) = norm_path.strip_prefix(&norm_root) {
            return render_named(rel);
        }
        if norm_path.is_absolute() {
            return render_relative(Path::new(
                norm_path.file_name().unwrap_or(norm_path.as_os_str()),
            ));
        }
    }
    render_relative(path)
}

fn render_named(rel: &Path) -> String {
    if rel.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        render_relative(rel)
    }
}

fn normalize_directory_prefix(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => normalize_existing_prefix(parent).join(name),
        _ => normalize_existing_prefix(path),
    }
}

pub fn render_relative(rel: &Path) -> String {
    rel.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}
