//! Every path below a site's `.tola` working root, and how long each one lives.
//!
//! Nothing below `.tola` is a site source: every path here is rebuilt, re-fetched, or discarded
//! on demand, so deleting the whole tree changes how long the next build takes and never what it
//! produces.

use std::path::{Path, PathBuf};

use super::INTERNAL_DIR;

/// The package mirrors `tola editor setup` publishes below this site's mirrors directory.
const PACKAGE_MIRROR_DIRECTORY: &str = "builtin-packages";

/// The directory holding every package mirror of one site.
const MIRRORS: &str = "mirrors";

/// One directory directly below the site's working root.
fn below_working_root(site: &Path, name: &str) -> PathBuf {
    site.join(INTERNAL_DIR).join(name)
}

/// Fetched remote module sources, at the commits `tola.lock` pins.
pub fn modules(site: &Path) -> PathBuf {
    below_working_root(site, "modules")
}

/// What a build keeps instead of rebuilding or downloading it again.
pub fn cache(site: &Path) -> PathBuf {
    below_working_root(site, "cache")
}

/// Encoded image variants, valid only while their bytes match the digest beside them.
pub fn image_cache(site: &Path) -> PathBuf {
    cache(site).join("images")
}

/// Downloaded remote icon collections, one file per digest the configuration pins.
pub fn icon_cache(site: &Path) -> PathBuf {
    cache(site).join("icons")
}

/// The package mirrors a site's editor reads directly.
pub fn mirrors(site: &Path) -> PathBuf {
    below_working_root(site, MIRRORS)
}

/// The embedded package sources `tola editor setup` publishes.
pub fn builtin_packages(site: &Path) -> PathBuf {
    mirrors(site).join(PACKAGE_MIRROR_DIRECTORY)
}

/// The mirrors as editor settings spell them: relative to the site root, so a site keeps its
/// settings when it moves.
pub fn builtin_packages_relative() -> PathBuf {
    // An empty site root leaves every component relative.
    mirrors(Path::new("")).join(PACKAGE_MIRROR_DIRECTORY)
}

/// The package view `tola editor setup` links a site's local packages into.
pub fn editor_packages(site: &Path) -> PathBuf {
    mirrors(site).join("editor-packages")
}

/// Directories one build owns until the work that created them ends.
pub fn scratch(site: &Path) -> PathBuf {
    below_working_root(site, "scratch")
}

/// One hook invocation's temporary directory, removed with the invocation.
pub fn hook_scratch(site: &Path) -> PathBuf {
    scratch(site).join("hooks")
}

/// The read-only upstream snapshot one generation round reads.
pub fn hook_candidates(site: &Path) -> PathBuf {
    scratch(site).join("candidates")
}

/// The files one `generate-outputs` command wrote, removed once they join the candidate.
pub fn hook_outputs(site: &Path) -> PathBuf {
    scratch(site).join("hook-outputs")
}

/// What a hook command keeps between runs; the command owns it, and Tola never reads or cleans it.
pub fn hook_cache(site: &Path) -> PathBuf {
    below_working_root(site, "hooks")
}

/// What one run leaves for another process to read: its logs and a development server's address.
pub fn state(site: &Path) -> PathBuf {
    below_working_root(site, "state")
}

/// Session logs a command appends to.
pub fn logs(site: &Path) -> PathBuf {
    state(site).join("logs")
}

/// Written once a development server's listener is bound and removed when it stops; a file a
/// killed server left behind names no server.
pub fn dev_server_address(site: &Path) -> PathBuf {
    state(site).join("dev-server.json")
}
