//! Build-side module facade: project configuration, resolution wrappers, and help text.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tola_input::InputMode;
use tola_module::module_set::{self, LoadedModule};

mod help;
pub mod hooks;
pub mod manifest;
pub mod query;
pub(crate) mod site;
mod watch;

#[cfg(test)]
mod tests;

pub use help::HELP;
pub use manifest::{
    find_initializer, nu_font_options, read_available_declarations, read_nu_modules,
};
pub use site::apply_module_icons;
pub use site::{LoadedSite, prepare_site};
pub use watch::{HookWatchEntry, prepare_hook_watch};

/// Resolve the declared modules' roots and read their manifests.
pub fn load_modules(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, LoadedModule>, module_set::ModuleLoadError> {
    let bundle = cancellation.bundle_cancellation();
    let roots = tola_module::inputs::module_roots_with_bundle(
        site_root,
        mode,
        vendor,
        declarations,
        &bundle,
    )?;
    module_set::load_modules_with_roots(site_root, declarations, &roots, mode, &[])
}

/// Read every declared module's manifest through a resolved root.
pub fn load_modules_with_roots(
    site_root: &Path,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    roots: &BTreeMap<String, PathBuf>,
    mode: InputMode,
) -> std::result::Result<BTreeMap<String, LoadedModule>, module_set::ModuleLoadError> {
    module_set::load_modules_with_roots(site_root, declarations, roots, mode, &[])
}

/// Command declarations remain available when unrelated build inputs need repair.
pub fn select_command_modules(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, tola_module::ModuleDeclaration>,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<
    BTreeMap<String, module_set::ModuleCommands>,
    tola_module::manifest::ManifestError,
> {
    let bundle = cancellation.bundle_cancellation();
    module_set::select_command_modules(site_root, vendor, declarations, mode, &bundle)
}
