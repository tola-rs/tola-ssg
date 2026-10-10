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
`tola vendor` freezes the external inputs selected by a production build into this directory.
Declare `path` before using the command:

```toml
[vendor]
path = \"vendor\"
```

`path` must be a real directory below the site root and outside `.tola`.

Tola prepares the dependencies, checks that the prepared copy builds under `--pure`, and only then
replaces the installed copy. Failed preparation keeps the previous inputs; an installation
interrupted before it committed is restored by the next `tola vendor`, and until then a build
refuses to read the vendored inputs and `--dry-run` refuses to run. Tola keeps its working files in
a sibling directory whose name starts with `.` and ends with `-vendor`: `vendor` above gives
`.vendor-vendor`.

Vendoring runs no hook stage — `before-build`, `generate-outputs`, and `after-publish` — and
publishes no site output. The files a `before-build` hook generates must already exist: produce
them with a normal build first. A `generate-outputs` hook's files are not frozen either; every
build produces them again, on this machine and on the one that builds from the vendored copy.

Tola replaces these three subdirectories as a whole; other files below `path` stay untouched:

- `typst-packages/` — resolved packages under `{namespace}/{name}/{version}/`, including packages
  selected from host roots or `--package-path`
- `icons/` — one `<namespace>.json` per configured `remote-json` collection
- `fonts/` — compiler fonts the build read that no `typst.fonts.paths` directory provides

Keep the site's own sources outside these directories. Packages of every namespace, including
`@preview`, resolve from vendor before host roots, and a remote icon collection prefers its
vendored copy — the only source a `--pure` build reads for it. Commit these inputs with the site:

```sh
tola vendor --dry-run   # prepare and verify without replacing vendor
tola vendor             # install the verified dependencies
tola build --pure       # build from site-owned inputs
```

`--refresh` does not read the installed vendor copy: every input is resolved again from the other
available sources, such as host package roots, caches, and the network. Use it to replace an
outdated copy, or to resolve the symbolic links `tola doctor` reports. A refresh that cannot
resolve an input keeps the previous copy. `--dry-run` leaves that copy unchanged but may fetch
dependencies, update caches, and wait for another site command.

`--offline` refuses Tola's network requests but permits host package roots and caches, and system
fonts when enabled. `--pure` also excludes those host inputs and source files physically outside
the site, including symlinks that leave it. Embedded packages and fonts stay available; derived
image caches may be used because their source bytes can still reproduce the result.

These flags restrict Tola's input reads, not hook commands or their toolchains: to reproduce the
whole build on another machine, pin each hook's command and the files its toolchain reads (its
manifest and lockfile), or commit the files it generates.";

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
