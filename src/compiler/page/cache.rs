//! Build VDOM cache for page compilation.
//!
//! Collects structural VDOM snapshots during build for persistence,
//! allowing `serve` to reuse VDOMs built by `build`.

use std::sync::LazyLock;

use crate::compiler::family::{CacheEntry, Indexed, SharedCache};
use tola_vdom::{CacheKey, Document};

/// Indexed VDOM document produced by compilation before cache projection.
pub type IndexedDocument = Document<Indexed>;

/// Global VDOM cache for build command
///
/// Collects structural VDOM snapshots during build for persistence.
/// This allows `serve` to reuse VDOMs built by `build`
pub static BUILD_CACHE: LazyLock<SharedCache> = LazyLock::new(SharedCache::new);

/// Store the structural projection of an indexed VDOM in the build cache.
pub fn cache_vdom(url_path: impl AsRef<str>, vdom: IndexedDocument) {
    let url = url_path.as_ref();
    let key = CacheKey::new(url);
    let entry = CacheEntry::with_default_version(tola_vdom::snapshot::project(&vdom));
    BUILD_CACHE.insert(key, entry);
    crate::debug!("cache"; "cache_vdom: url={}, cache_size={}", url, BUILD_CACHE.len());
}
