//! Declarations, resolution, locks, and the store that makes a build reproducible.
//!
//! A declaration is what the author wrote. Choosing roots turns it into a concrete source, and the
//! lock records that fact together with the integrity of the bytes. Only `vendor/` holds those
//! bytes; `.tola/` and host caches are performance state a build may drop at any time.

pub mod hash;
pub mod lock;
pub mod mode;
pub mod module;

pub use hash::{FileHashError, Sha256Digest, TreeHashError, file_sha256, tree_sha256};
pub use lock::{
    IconSource, InputLock, LOCK_FILE, LockError, LockedIcon, LockedModule, LockedPackage,
    LockedRevision,
};
pub use mode::InputMode;
pub use module::{ModuleRootError, declaration_root, module_roots, record_module_roots};
