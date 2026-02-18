//! Build VDOM cache for page compilation.
//!
//! Collects indexed VDOMs during build for persistence,
//! allowing `serve` to reuse VDOMs built by `build`.

use std::sync::LazyLock;

use crate::compiler::family::{CacheEntry, Indexed, SharedCache};
use tola_vdom::{CacheKey, Document};

/// Indexed VDOM document for hot reload diffing
pub type IndexedDocument = Document<Indexed>;

/// Global VDOM cache for build command
///
/// Collects indexed VDOMs during build for persistence
/// This allows `serve` to reuse VDOMs built by `build`
pub static BUILD_CACHE: LazyLock<SharedCache> = LazyLock::new(SharedCache::new);

/// Store an indexed VDOM in the build cache
pub fn cache_vdom(url_path: impl AsRef<str>, vdom: IndexedDocument) {
    let url = url_path.as_ref();
    let key = CacheKey::new(url);
    let entry = CacheEntry::with_default_version(vdom);
    BUILD_CACHE.insert(key, entry);
    crate::debug!("cache"; "cache_vdom: url={}, cache_size={}", url, BUILD_CACHE.len());
}
