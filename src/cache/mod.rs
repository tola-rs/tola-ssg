//! Cache persistence for VDOM and compile diagnostics.

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

// Diagnostics state (errors + warnings)
pub use failure::{
    PersistedDiagnostics, PersistedError, PersistedWarning, persist_diagnostics,
    restore_diagnostics,
};

// Modified file detection
pub use modified::{get_modified_files, get_source_paths};
