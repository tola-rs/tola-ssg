//! Unified hashing utilities using FxHash.
//!
//! Uses `rustc_hash::FxHasher` for:
//! - Fast, deterministic hashing (optimized for small data)
//! - No extra dependencies (rustc_hash already used for FxHashSet/FxHashMap)
//!
//! # Usage
//!
//! ```ignore
//! use crate::utils::hash;
//!
//! let h = hash::compute("some content"); // -> u64
//! let fp = hash::fingerprint("some content"); // -> "a1b2c3d4"
//! ```

use rustc_hash::FxHasher;
use std::hash::Hasher;
use std::io::{self, Read};

/// Compute 64-bit hash from byte data.
#[inline]
pub fn compute<T: AsRef<[u8]> + ?Sized>(data: &T) -> u64 {
    let mut hasher = FxHasher::default();
    hasher.write(data.as_ref());
    hasher.finish()
}

/// Compute hash from a reader (streaming, for large files).
pub fn compute_reader(mut reader: impl Read) -> io::Result<u64> {
    let mut hasher = FxHasher::default();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.write(&buffer[..n]);
    }
    Ok(hasher.finish())
}

/// Compute hash and return as 8-char hex fingerprint.
///
/// Useful for cache-busting filenames (e.g. `style.a1b2c3d4.css`).
#[inline]
pub fn fingerprint<T: AsRef<[u8]> + ?Sized>(value: &T) -> String {
    format!("{:016x}", compute(value))[..8].to_string()
}
