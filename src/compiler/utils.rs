use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// Files to ignore during directory traversal
pub const IGNORED_FILES: &[&str] = &[".DS_Store"];

/// Collect all files from a directory recursively
pub fn collect_all_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_str().unwrap_or_default();
            !IGNORED_FILES.contains(&name)
        })
        .map(|e| e.into_path())
        .collect()
}

/// Check if destination is up-to-date compared to source and dependencies
pub fn is_up_to_date(src: &Path, dst: &Path, deps_mtime: Option<SystemTime>) -> bool {
    let Ok(src_meta) = src.metadata() else {
        return false;
    };
    let Ok(dst_meta) = dst.metadata() else {
        return false;
    };

    let Ok(src_time) = src_meta.modified() else {
        return false;
    };
    let Ok(dst_time) = dst_meta.modified() else {
        return false;
    };

    // Check if source is newer than destination
    if src_time > dst_time {
        return false;
    }

    // Check if any dependency is newer than destination
    if let Some(deps) = deps_mtime
        && deps > dst_time
    {
        return false;
    }

    true
}
