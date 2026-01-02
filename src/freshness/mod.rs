//! Freshness detection: content-hash (blake3) for sources, mtime for outputs.
#![allow(dead_code)]

mod cache;
mod hash;
pub mod mtime;

pub use cache::clear_cache;
pub use hash::{ContentHash, build_hash_marker, compute_deps_hash, compute_file_hash, is_fresh};
pub use mtime::is_newer_than;
