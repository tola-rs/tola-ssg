//! Encoded variants retained between builds, keyed by the request that produced them.
//!
//! A retained variant is published only after its bytes match the digest recorded beside them, so
//! a truncated or damaged file is a miss rather than a different image. Nothing is ever removed:
//! the directory holds derived bytes only, so deleting it costs a render and nothing else.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tola_typst::ContentDigest;

use crate::filesystem::atomic_write;

/// Encoded variants stored independently of published revisions.
pub(crate) struct VariantCache {
    directory: PathBuf,
}

impl VariantCache {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// The stored bytes of one requested variant, when a verified copy is present.
    pub(crate) fn read(&self, key: ContentDigest) -> Option<Arc<[u8]>> {
        let digest = fs::read_to_string(self.directory.join(key.to_hex())).ok()?;
        let digest = digest.trim();
        if !is_digest_hex(digest) {
            return None;
        }
        let bytes = fs::read(self.directory.join(digest)).ok()?;
        (ContentDigest::of(&bytes).to_hex() == digest).then(|| Arc::from(bytes))
    }

    pub(crate) fn store(&self, key: ContentDigest, bytes: &[u8]) -> Result<()> {
        let content = ContentDigest::of(bytes).to_hex();
        // The bytes land first, so a recorded pointer never names a missing file.
        atomic_write(&self.directory.join(&content), bytes).with_context(|| {
            "Tola could not store a resized image; check that the site's `.tola` directory can be written"
        })?;
        atomic_write(&self.directory.join(key.to_hex()), content.as_bytes()).with_context(|| {
            "Tola could not store a resized image; check that the site's `.tola` directory can be written"
        })
    }
}

/// Whether a pointer holds one hexadecimal digest, so it can only name a file beside it.
fn is_digest_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn cache() -> (TempDir, VariantCache) {
        let directory = TempDir::new().unwrap();
        let cache = VariantCache::new(directory.path().to_path_buf());
        (directory, cache)
    }

    #[test]
    fn stored_variant_is_read_back() {
        let (_directory, cache) = cache();
        let key = ContentDigest::of(b"request");
        cache.store(key, b"encoded").unwrap();
        assert_eq!(
            cache.read(key).as_deref(),
            Some(b"encoded".as_slice()),
            "a stored variant survives a fresh lookup"
        );
    }

    #[test]
    fn absent_variant_misses() {
        let (_directory, cache) = cache();
        assert!(cache.read(ContentDigest::of(b"never stored")).is_none());
    }

    #[test]
    fn damaged_bytes_miss() {
        let (_directory, cache) = cache();
        let key = ContentDigest::of(b"request");
        cache.store(key, b"encoded").unwrap();
        let pointer = std::fs::read_to_string(_directory.path().join(key.to_hex())).unwrap();
        std::fs::write(_directory.path().join(pointer.trim()), b"damaged").unwrap();
        assert!(cache.read(key).is_none());
    }

    /// A pointer naming anything but a digest in this directory is not followed.
    #[test]
    fn foreign_pointer_misses() {
        let (directory, cache) = cache();
        let key = ContentDigest::of(b"request");
        std::fs::write(directory.path().join(key.to_hex()), b"../../etc/passwd").unwrap();
        assert!(cache.read(key).is_none());
    }
}
