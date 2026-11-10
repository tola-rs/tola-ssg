//! Tola's input layer: checksums, lock entries, filesystem identities, and cancellable reads.
//!
//! The lock records the selector, the commit it resolved to, and the digest of the exact bytes.
//! A source identity records a logical path and the physical path it resolves to, and every
//! digest can be recomputed. A lock from an older format fails with a named repair instead of
//! being read as if it still described this site.

#![forbid(unsafe_code)]

mod access;
mod hash;
mod identity;
pub mod layout;
mod lock;
mod mode;
mod path;
mod read;
mod selection;

pub use access::{InputScope, NetworkAccess};
pub use hash::{FileHashError, Sha256Digest, TreeHashError, file_sha256, tree_sha256};
pub use identity::{FilesystemSourceIdentity, paths_intersect};
pub use layout::INTERNAL_DIR;
pub use lock::{
    IconSource, InputLock, LOCK_FILE, LockError, LockedIcon, LockedModule, LockedPackage,
    LockedRevision, TarballLocation,
};
pub use mode::InputMode;
pub use path::{lexical_path_identity, normalize_existing_prefix, normalize_path};
pub use read::{Cancelled, FileReadError, read_chunks, read_file_chunks};
pub use selection::{FilePatterns, FileSelection, PatternError};
