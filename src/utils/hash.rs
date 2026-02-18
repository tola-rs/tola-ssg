//! Unified deterministic hashing utilities
//!
//! Provides cross-process deterministic hashing using blake3.
//! This module should be used instead of `std::collections::hash_map::DefaultHasher`
//! which uses a random seed and is NOT deterministic across process restarts.
//!
//! # Why not DefaultHasher or FxHasher?
//!
//! - `DefaultHasher` uses SipHash with a random seed initialized at program start.
//! - `FxHasher` is fast but implementation-dependent - not guaranteed stable.
//! - `blake3` is cryptographic, fast, and produces identical output everywhere.
//!
//! # Usage
//!
//! ```ignore
//! use crate::utils::hash::{StableHasher, hash_bytes, hash_str};
//!
//! // Simple hashing
//! let h = hash_str("hello");
//!
//! // Builder pattern for complex hashing
//! let h = StableHasher::new()
//!     .update_str("tag")
//!     .update_u64(42)
//!     .finish();
//! ```

use std::io::{self, Read};

// =============================================================================
// StableHasher - Builder Pattern
// =============================================================================

/// A deterministic hasher using blake3
///
/// Unlike `std::hash::Hasher`, this produces the same output across
/// process restarts for the same input
pub struct StableHasher {
    inner: blake3::Hasher,
}

impl StableHasher {
    /// Create a new StableHasher
    #[inline]
    pub fn new() -> Self {
        Self {
            inner: blake3::Hasher::new(),
        }
    }

    /// Update with raw bytes
    #[inline]
    pub fn update(mut self, data: &[u8]) -> Self {
        self.inner.update(data);
        self
    }

    /// Update with a string
    #[inline]
    pub fn update_str(self, s: &str) -> Self {
        self.update(s.as_bytes())
    }

    /// Update with a u64 value (little-endian)
    #[inline]
    #[allow(dead_code)]
    pub fn update_u64(self, v: u64) -> Self {
        self.update(&v.to_le_bytes())
    }

    /// Update with a usize value (little-endian)
    #[inline]
    #[allow(dead_code)]
    pub fn update_usize(self, v: usize) -> Self {
        self.update(&v.to_le_bytes())
    }

    /// Finish and return the hash as u64
    ///
    /// Takes the first 8 bytes of blake3 output as little-endian u64.
    #[inline]
    pub fn finish(self) -> u64 {
        let hash = self.inner.finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
    }
}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Convenience functions
// =============================================================================

/// Compute 64-bit deterministic hash from byte data
#[inline]
pub fn compute<T: AsRef<[u8]> + ?Sized>(data: &T) -> u64 {
    hash_bytes(data.as_ref())
}

/// Hash a byte slice to u64
#[inline]
pub fn hash_bytes(data: &[u8]) -> u64 {
    StableHasher::new().update(data).finish()
}

/// Hash a string to u64
#[inline]
#[allow(dead_code)]
pub fn hash_str(s: &str) -> u64 {
    hash_bytes(s.as_bytes())
}

/// Generate a short fingerprint string (8 hex chars) from content
#[inline]
pub fn fingerprint(s: &str) -> String {
    format!("{:08x}", hash_str(s) as u32)
}

/// Hash multiple strings together
#[inline]
#[allow(dead_code)]
pub fn hash_strs(strs: &[&str]) -> u64 {
    let mut hasher = StableHasher::new();
    for s in strs {
        hasher = hasher.update_str(s);
    }
    hasher.finish()
}

/// Compute hash from a reader (streaming, for large files)
#[allow(dead_code)]
pub fn compute_reader(mut reader: impl Read) -> io::Result<u64> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let hash = hasher.finalize();
    Ok(u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap()))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic() {
        // Same input should always produce same output
        let h1 = hash_str("hello world");
        let h2 = hash_str("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_different_inputs() {
        let h1 = hash_str("hello");
        let h2 = hash_str("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_builder_pattern() {
        let h = StableHasher::new()
            .update_str("tag")
            .update_u64(42)
            .update_usize(100)
            .finish();

        // Should be reproducible
        let h2 = StableHasher::new()
            .update_str("tag")
            .update_u64(42)
            .update_usize(100)
            .finish();

        assert_eq!(h, h2);
    }

    #[test]
    fn test_order_matters() {
        let h1 = StableHasher::new().update_str("a").update_str("b").finish();

        let h2 = StableHasher::new().update_str("b").update_str("a").finish();

        assert_ne!(h1, h2);
    }
}
