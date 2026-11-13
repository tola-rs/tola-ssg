//! `[vendor]` section configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tola_config::Config;

use crate::config::ConfigDiagnostics;

/// Keep the modules, packages, and icon collections a build reads with the site.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "vendor")]
pub struct VendorConfig {
    /// Directory where `tola vendor` copies dependencies, relative to the site root.
    /// Set it before running `tola vendor`. There is no default directory.
    pub path: Option<PathBuf>,
}

impl VendorConfig {
    /// What `tola help "[vendor]"` adds under its table.
    pub const HELP: &'static str = r#"
Set a directory below the site root in `tola.toml`.

```toml
[vendor]
path = "vendor"
```

From the site root, run `tola vendor`, then `tola build --pure` to check that the
copied dependencies work. Commit `vendor/` and `tola.lock` with the site. Tola checks
the files before replacing these subdirectories:

- `modules/` holds locked remote modules by alias.
- `typst-packages/` holds packages by namespace, name, and version.
- `icons/` holds one JSON file per remote icon collection.

Keep site fonts in `[typst.fonts] paths`. Tola does not copy compiler fonts into vendor.
The vendor directory must stay outside `.tola`. A failed preparation keeps the previous
copy. If a replacement was interrupted, rerun `tola vendor` before building.

Use `tola vendor --dry-run` to prepare and check dependencies without replacing the copy.
Use `tola vendor --refresh` to refresh it from the configured sources. A failed refresh
keeps the previous copy.

If the site needs inputs from `before-build`, generate them before vendoring.
Use `tola check` to check site links, including links to generated files.

`--offline` blocks Tola's network access. `--pure` also excludes host packages, caches,
system fonts, and paths outside the site, including through symlinks. Embedded packages
and fonts stay available. Hooks run with their own tools and permissions."#;

    /// The directory a site keeps its vendored module sources in.
    pub fn modules(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.join("modules"))
    }

    /// The directory one declared module's frozen contents live in.
    pub fn module(&self, alias: &str) -> Option<PathBuf> {
        self.modules().map(|modules| modules.join(alias))
    }

    /// The directory a site keeps its vendored Typst packages in.
    pub fn typst_packages(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.join("typst-packages"))
    }

    /// The directory a site keeps its vendored icon collections in.
    pub fn icons(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.join("icons"))
    }

    /// The file one vendored icon collection is frozen to.
    pub fn icon_collection(&self, namespace: &str) -> Option<PathBuf> {
        self.icons()
            .map(|icons| icons.join(format!("{namespace}.json")))
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
        let physical_root = tola_input::normalize_existing_prefix(root);
        let physical_path = tola_input::normalize_existing_prefix(path);
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
                    tola_diagnostic::display_path(path, root)
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
    fn internal_directory_reserved_any_spelling() {
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
