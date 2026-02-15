//! Cache persistence for VDOM and compile errors.

mod failure;
mod index;
mod modified;
mod vdom;

/// Cache directory name (inside project root)
pub(crate) const CACHE_DIR: &str = ".tola/cache";

// VDOM cache
pub use vdom::{
    clear_cache_dir, has_cache, persist_cache, restore_cache, restore_dependency_graph,
};

// Failure state
pub use failure::{PersistedError, PersistedErrorState, persist_errors, restore_errors};

// Modified file detection
pub use modified::{get_modified_files, get_source_paths};
