//! `[vendor]` section configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tola_config::Config;

use crate::config::ConfigDiagnostics;

/// Keep packages, fonts, and icons with the site for offline builds on other machines.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "vendor")]
pub struct VendorConfig {
    /// Directory, relative to the site root, where `tola vendor` freezes the inputs a build
    /// selected.
    pub path: Option<PathBuf>,
}

impl VendorConfig {
    /// What `tola help config vendor` adds under its table.
    pub const HELP: &'static str = "\
`tola vendor` freezes the external inputs selected by a production build into this directory.
Declare `path` before using the command:

```toml
[vendor]
path = \"vendor\"
```

It runs no hooks and publishes no site output. Generated site inputs must already be available
for that build. Tola prepares the dependencies, checks that the prepared copy builds under
`--pure`, and only then replaces the installed copy. Failed preparation keeps the previous inputs.

Three subdirectories belong to Tola; other files below `path` remain yours:

- `typst-packages/` — resolved packages under `{namespace}/{name}/{version}/`, including packages
  selected from host roots or `--package-path`
- `icons/` — one `<namespace>.json` per configured `remote-json` collection
- `fonts/` — compiler fonts the build read that no `typst.fonts.paths` directory provides

Packages of every namespace, including `@preview`, resolve from vendor before host roots. A
remote icon collection also prefers its vendored copy. Commit these inputs with the site:

```sh
tola vendor --dry-run   # prepare and verify without replacing vendor
tola vendor             # install the verified dependencies
tola build --pure       # build from site-owned inputs
```

`--refresh` selects again without reading the installed vendor copy. `--dry-run` leaves that
copy unchanged but may fetch dependencies, update caches, and wait for another site command.

`--offline` refuses Tola's network requests but permits host package roots and caches, and system
fonts when enabled. `--pure` also excludes those host inputs and source files physically outside
the site, including symlinks that leave it. Embedded packages and fonts stay available; derived
image caches may be used because their source bytes can still reproduce the result.

These flags restrict Tola's input reads, not hook scripts or their toolchains. Keep a hook's tools
and dependencies pinned separately when the whole build must reproduce on another machine.";

    /// The directory a site keeps its vendored Typst packages in.
    pub fn typst_packages(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.join("typst-packages"))
    }

    /// The directory a site keeps its vendored icon collections in.
    pub fn icons(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.join("icons"))
    }

    /// The directory a site keeps its vendored compiler fonts in.
    pub fn fonts(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.join("fonts"))
    }

    /// The file one vendored icon collection is frozen to.
    pub fn icon_collection(&self, namespace: &str) -> Option<PathBuf> {
        self.icons()
            .map(|icons| icons.join(format!("{namespace}.json")))
    }

    /// Recovery stays beside the frozen inputs, independent of disposable `.tola` state.
    pub fn workspace_path(&self) -> Option<PathBuf> {
        let path = self.path.as_ref()?;
        let mut name = std::ffi::OsString::from(".");
        name.push(path.file_name()?);
        name.push("-vendor");
        Some(path.with_file_name(name))
    }

    /// Validate the path before site-root normalization.
    pub fn validate_paths(&self, diagnostics: &mut ConfigDiagnostics) {
        let Some(path) = &self.path else {
            return;
        };
        let accepted = super::path::validate_site_relative_path(
            path,
            Self::FIELDS.path.as_str(),
            Self::FIELDS.path,
            diagnostics,
        );
        if !accepted {
            return;
        }
        if !path
            .components()
            .any(|part| matches!(part, std::path::Component::Normal(_)))
        {
            diagnostics.error(
                Self::FIELDS.path,
                "`vendor.path` must name a directory below the site root",
            );
            return;
        }
        if super::path::enters_internal_directory(path) {
            diagnostics.error_with_help(
                Self::FIELDS.path,
                format!(
                    "`vendor.path` `{}` is inside Tola's `.tola` directory",
                    super::path::declared_path(path)
                ),
                "write a directory outside `.tola`",
            );
        }
    }

    /// Input aliases must not turn the vendor directory into machine or disposable state.
    pub(crate) fn validate_internal_boundaries(
        &self,
        root: &Path,
        diagnostics: &mut ConfigDiagnostics,
    ) {
        let Some(path) = &self.path else {
            return;
        };
        let physical_root = crate::filesystem::normalize_existing_prefix(root);
        let physical_path = crate::filesystem::normalize_existing_prefix(path);
        if physical_path == physical_root || !physical_path.starts_with(&physical_root) {
            diagnostics.error(
                Self::FIELDS.path,
                "`vendor.path` must stay below the site root after resolving links",
            );
        }
        if super::path::reaches_internal_directory(root, path) {
            diagnostics.error(
                Self::FIELDS.path,
                format!(
                    "`vendor.path` `{}` is inside Tola's `.tola` directory",
                    crate::filesystem::display_path(path, root)
                ),
            );
        }
    }

    /// Normalize the path relative to the site root.
    pub fn normalize(&mut self, root: &Path) {
        if let Some(path) = &mut self.path {
            *path = root.join(&*path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigDiagnostics;

    #[test]
    fn vendor_paths_stay_below_site() {
        for declared in [".tola/vendor", "../vendor", "."] {
            let config = VendorConfig {
                path: Some(PathBuf::from(declared)),
            };
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate_paths(&mut diagnostics);
            assert!(
                diagnostics.into_result().is_err(),
                "{declared} must be rejected"
            );
        }
    }

    #[test]
    fn internal_directory_is_reserved_in_any_spelling() {
        let site = tempfile::TempDir::new().unwrap();
        for declared in [".tola/vendor", ".TOLA/vendor", ".Tola/vendor"] {
            let config = VendorConfig {
                path: Some(PathBuf::from(declared)),
            };
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate_paths(&mut diagnostics);
            assert!(
                diagnostics.into_result().is_err(),
                "{declared} must be rejected"
            );

            let resolved = VendorConfig {
                path: Some(site.path().join(declared)),
            };
            let mut diagnostics = ConfigDiagnostics::new();
            resolved.validate_internal_boundaries(site.path(), &mut diagnostics);
            assert!(
                diagnostics.into_result().is_err(),
                "{declared} must be rejected after resolution"
            );
        }
    }

    #[test]
    fn ordinary_vendor_paths_are_accepted() {
        for declared in ["vendor", "vendorfiles", "vendor/tola", ".vendorfiles"] {
            let config = VendorConfig {
                path: Some(PathBuf::from(declared)),
            };
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate_paths(&mut diagnostics);
            assert!(
                diagnostics.into_result().is_ok(),
                "{declared} must be accepted"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn vendor_alias_cannot_escape_site() {
        let site = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let path = site.path().join("vendor");
        std::os::unix::fs::symlink(outside.path(), &path).unwrap();
        let config = VendorConfig { path: Some(path) };
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate_internal_boundaries(site.path(), &mut diagnostics);
        assert!(diagnostics.into_result().is_err());
    }
}
