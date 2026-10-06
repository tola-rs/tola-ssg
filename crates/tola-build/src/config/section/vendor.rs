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
    /// What `tola help "[vendor]"` adds under its table.
    pub const HELP: &'static str = "\
`tola vendor` freezes the external inputs a build selected into `path`, so another machine can
build the same site without network access. Tola searches that directory before every host root,
so the offline build reproduces the online one:

```toml
[vendor]
path = \"vendor\"
```

Three subdirectories under `path` belong to Tola; every other file there is yours:

- `typst-packages/` — every package the build resolved, under `{namespace}/{name}/{version}/`
- `icons/` — one `<namespace>.json` per configured `remote-json` collection
- `fonts/` — the compiler fonts the build read that no configured `typst.fonts.paths` directory
  contains

A package the build selected from a host root or from `--package-path` is copied in the same way,
so a later `--pure` build resolves it from the site itself. Every namespace resolves from here
first, `@preview` among them. The site above ends up holding:

```text
vendor/
  typst-packages/preview/cetz/0.3.4/
  icons/brand.json
  fonts/
```

A machine without network access builds from them:

```sh
tola vendor          # on a machine with network access; commit the result
tola build --pure    # in CI: the site's own inputs only
```

Two options cover the common cases: `tola vendor --refresh` resolves again without reading the
existing copy and replaces it after validation, and `tola vendor --dry-run` prepares and validates
without touching `path`.

`--offline` refuses Tola's network requests but still resolves host package roots, local caches, and
system fonts. `--pure` refuses those too, and source files outside the site, so an import with no
vendored copy fails instead of downloading. Neither flag sandboxes a hook command.";

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
