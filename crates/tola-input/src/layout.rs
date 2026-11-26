//! Paths under a site's `.tola` working root.
//!
//! Everything under `.tola` is disposable/reproducible and must not affect build outputs.

use std::path::{Path, PathBuf};

/// Disposable working directory excluded from formal site inputs.
pub const INTERNAL_DIR: &str = ".tola";

const PACKAGE_MIRROR_DIR: &str = "builtin-packages";
const MIRRORS_DIR: &str = "mirrors";

fn internal_subdir(site: &Path, name: &str) -> PathBuf {
    site.join(INTERNAL_DIR).join(name)
}

/// Cached remote module directories, keyed by locked commit and subdirectory.
pub fn modules(site: &Path) -> PathBuf {
    internal_subdir(site, "modules")
}

/// Reusable build and fetch caches.
pub fn cache(site: &Path) -> PathBuf {
    internal_subdir(site, "cache")
}

/// Encoded image variant cache.
pub fn image_cache(site: &Path) -> PathBuf {
    cache(site).join("images")
}

/// Downloaded remote icon collection cache.
pub fn icon_cache(site: &Path) -> PathBuf {
    cache(site).join("icons")
}

/// Editor-visible package mirrors.
pub fn mirrors(site: &Path) -> PathBuf {
    internal_subdir(site, MIRRORS_DIR)
}

/// Embedded package sources published by `tola editor setup`.
pub fn builtin_packages(site: &Path) -> PathBuf {
    mirrors(site).join(PACKAGE_MIRROR_DIR)
}

/// Relative path to embedded package mirrors from the site root.
pub fn builtin_packages_relative() -> PathBuf {
    mirrors(Path::new("")).join(PACKAGE_MIRROR_DIR)
}

/// Local package links created by `tola editor setup`.
pub fn editor_packages(site: &Path) -> PathBuf {
    mirrors(site).join("editor-packages")
}

/// Per-build scratch directories cleaned up when the run ends.
pub fn scratch(site: &Path) -> PathBuf {
    internal_subdir(site, "scratch")
}

/// Temporary directory for hook invocations.
pub fn hook_scratch(site: &Path) -> PathBuf {
    scratch(site).join("hooks")
}

/// Read-only upstream snapshot passed to generation hooks.
pub fn hook_candidates(site: &Path) -> PathBuf {
    scratch(site).join("candidates")
}

/// Staging directory for `generate-outputs` command outputs.
pub fn hook_outputs(site: &Path) -> PathBuf {
    scratch(site).join("hook-outputs")
}

/// Persistent cache directory owned by hook commands.
pub fn hook_cache(site: &Path) -> PathBuf {
    internal_subdir(site, "hooks")
}

/// Runtime state directory (logs, dev-server address).
pub fn state(site: &Path) -> PathBuf {
    internal_subdir(site, "state")
}

/// Session log directory.
pub fn logs(site: &Path) -> PathBuf {
    state(site).join("logs")
}

/// Dev server address file written while the listener is bound.
pub fn dev_server_address(site: &Path) -> PathBuf {
    state(site).join("dev-server.json")
}
