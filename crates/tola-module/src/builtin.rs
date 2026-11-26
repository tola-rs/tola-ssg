//! Module files embedded by the application.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tola_address::OutputPath;
use tola_input::{Sha256Digest, layout};

/// The name and files of a site module shipped by the application.
#[derive(PartialEq, Eq)]
pub struct BuiltinModule {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static [u8])],
}

static MODULES: OnceLock<&'static [BuiltinModule]> = OnceLock::new();

/// Install the embedded modules before reading site declarations.
pub fn install_builtin_modules(modules: &'static [BuiltinModule]) -> Result<(), &'static str> {
    let mut names = BTreeSet::new();
    for pkg in modules {
        if OutputPath::parse(pkg.name).is_err()
            || pkg.name.contains('/')
            || !names.insert(tola_address::portable_collision_key(pkg.name))
        {
            return Err("built-in modules need distinct names without directory separators");
        }
        let mut paths = BTreeSet::new();
        for (file, _) in pkg.files {
            let path = OutputPath::parse(file).map_err(|_| "invalid built-in module file name")?;
            if !paths.insert(path.portable_key()) {
                return Err("built-in module files need distinct paths");
            }
        }
        if !pkg
            .files
            .iter()
            .any(|(file, _)| *file == "tola-module.toml")
        {
            return Err("a built-in module needs a `tola-module.toml` manifest");
        }
    }
    if *MODULES.get_or_init(|| modules) != modules {
        return Err("Tola's built-in module files were already installed");
    }
    Ok(())
}

impl BuiltinModule {
    pub(crate) fn named(name: &str) -> Option<&'static Self> {
        MODULES.get()?.iter().find(|pkg| pkg.name == name)
    }

    pub(crate) fn root(&self, site: &Path) -> PathBuf {
        let mut entries = String::new();
        for (path, bytes) in self.files {
            writeln!(entries, "{path} {}", Sha256Digest::of_bytes(bytes)).unwrap();
        }
        layout::modules(site)
            .join("builtin")
            .join(self.name)
            .join(Sha256Digest::of_bytes(entries.as_bytes()).to_string())
    }
}
