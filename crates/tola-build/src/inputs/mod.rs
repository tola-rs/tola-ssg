//! Module roots: where one declared module reads, and the lock facts that fix it.

pub mod module;

pub use module::{declaration_root, module_roots, record_module_roots, refresh_module_roots};
pub use tola_module::inputs::ModuleRootError;
