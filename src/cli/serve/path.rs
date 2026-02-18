//! URL to filesystem path resolution.

use std::path::{Path, PathBuf};

/// Resolve URL to filesystem path, handling index.html for directories
pub fn resolve_path(url: &str, serve_root: &Path) -> Option<PathBuf> {
    let clean = normalize_url(url);

    // Reject paths with suspicious patterns early
    if clean.contains("..") {
        return None;
    }

    let local = serve_root.join(&clean);

    // Canonicalize to resolve symlinks and verify path is under serve_root
    // This prevents traversal via symlinks or encoded sequences
    let canonical = local.canonicalize().ok()?;
    let root_canonical = serve_root.canonicalize().ok()?;

    if !canonical.starts_with(&root_canonical) {
        // Path escapes serve_root - reject
        return None;
    }

    if canonical.is_file() {
        return Some(canonical);
    }

    if canonical.is_dir() {
        let index = canonical.join("index.html");
        if index.is_file() {
            return Some(index);
        }
    }

    None
}

/// Normalize URL: decode, strip query string, trim slashes
fn normalize_url(url: &str) -> String {
    use percent_encoding::percent_decode_str;
    let decoded = percent_decode_str(url)
        .decode_utf8()
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_default();

    let path = decoded.split('?').next().unwrap_or(&decoded);
    path.trim_matches('/').to_string()
}
