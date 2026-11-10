//! Build-side module root wrappers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tola_input::InputMode;
use tola_module::declaration::ModuleSource;
use tola_module::inputs::ModuleRootError;
use tola_module::inputs::{
    declaration_root as resolve_declaration_root, module_roots_with_bundle,
    record_module_roots_with_bundle, refresh_module_roots_with_bundle,
};

/// The directory every declared module reads.
pub fn module_roots(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    module_roots_with_bundle(
        site_root,
        mode,
        vendor,
        declarations,
        &cancellation.bundle_cancellation(),
    )
}

/// Record the root and digest of every declared module into `lock`, fetching when allowed.
pub fn record_module_roots(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    lock: &mut tola_input::InputLock,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    record_module_roots_with_bundle(
        site_root,
        mode,
        vendor,
        declarations,
        lock,
        &cancellation.bundle_cancellation(),
    )
}

/// Forget the fact of every selected module, then resolve and record it again.
pub fn refresh_module_roots(
    site_root: &Path,
    mode: InputMode,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    lock: &mut tola_input::InputLock,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, PathBuf>, ModuleRootError> {
    refresh_module_roots_with_bundle(
        site_root,
        mode,
        vendor,
        declarations,
        lock,
        &cancellation.bundle_cancellation(),
    )
}

/// The directory one declaration points at, without fetching or verifying it.
pub fn declaration_root(
    site_root: &Path,
    vendor: Option<&Path>,
    alias: &str,
    source: &ModuleSource,
) -> std::result::Result<PathBuf, ModuleRootError> {
    resolve_declaration_root(site_root, vendor, alias, source)
}
