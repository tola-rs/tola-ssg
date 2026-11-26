//! Icon collection sources available to `@tola/icon` during Bundle compilation.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tola_config::Config;

use crate::config::ConfigDiagnostics;

/// Import `icon` from `@tola/icon:0.0.0`; `icon("namespace:name")` inserts an SVG in HTML.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config, PartialEq, Eq)]
#[serde(default)]
#[config(section = "icons")]
pub struct IconsConfig {
    /// Each namespace names where its icons come from: a local SVG tree or IconifyJSON file, or
    /// a remote collection named by an npm-published preset or by a URL.
    #[config(fragment_with = crate::config::fragment::icon_collections)]
    pub collections: BTreeMap<String, IconCollectionSource>,
}

pub use tola_module::icons::IconCollectionSource;

impl IconsConfig {
    pub(crate) fn validate(&self, diagnostics: &mut ConfigDiagnostics) {
        for (namespace, source) in &self.collections {
            if namespace.parse::<tola_icons::IconCollectionName>().is_err() {
                diagnostics.error_with_help(
                    Self::FIELDS.collections,
                    format!("icon collection namespace `{namespace}` is not usable"),
                    "Use 1–512 ASCII letters, digits, hyphens, or underscores, starting with a letter or digit and not ending with a hyphen",
                );
            }
            if let Err(message) = source.validate() {
                diagnostics.error(
                    Self::FIELDS.collections,
                    format!("icon collection `{namespace}`: {message}"),
                );
            }
        }
    }

    /// What `tola help "[icons]"` adds under its table.
    pub const HELP: &'static str = r#"
### Use an Iconify collection

Add Lucide to `tola.toml`.

```toml
[icons.collections.lucide]
source-type = "remote-json"
preset = "lucide"
```

From the site root, run `tola update lucide` to fetch and lock the collection. Run
`tola inspect icons lucide --interactive` to browse its names. The table name `lucide`
supplies the prefix in `lucide:rocket`.

In a site created with `tola init`, add this to `content/index.typ`.

```typst
#import "@tola/icon:0.0.0": icon
#icon("lucide:rocket") Launch notes
```

Run `tola dev` and open the printed URL. The home page shows a rocket next to “Launch notes”.
Run `tola help "@tola/icon" icon-url` to use a separate SVG file in an image element.

To use the prefix `ui`, name the table `[icons.collections.ui]` and keep
`preset = "lucide"`. Run `tola update ui` and change the Typst call to `icon("ui:rocket")`.

A preset names an `@iconify-json` package. Omit `version` to select the latest release
on first resolution, or set `version` to pin the package version. Tola verifies the registry
tarball with its published integrity. Optional `mirrors` are tried in order if the registry
tarball cannot be downloaded. A mirror must serve identical bytes and can use `{name}` and
`{version}` in its URL.

For a collection outside the registry, write its JSON `url` instead of `preset` and `version`.
Write each mirror URL in full. The first download is recorded in `tola.lock` and subsequent
builds verify it. Commit `tola.lock` with the site.

Run `tola vendor` to copy remote collections to `icons/<namespace>.json` under the
configured vendor directory. A `--pure`
build reads those copies.

### Module collections

A module declares its own `[icons.collections]` in `tola-module.toml`. Local paths
start at the module root. Pass `module:` to `icon`, `icon-bytes`, or `icon-url` to use
that instance's collection. Inside a module, use `icon.with(module: get-module().name)`.

Example: display an icon from a local module:

Copy the `mark.svg` from the local SVG example below to `theme/icons/brand/mark.svg`.
Save this as `theme/tola-module.toml`:

```toml
[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons/brand"
```

Add the module to the site's `tola.toml`:

```toml
[modules.notes]
path = "theme"
```

From the site root, run `tola inspect icons brand --module notes`. The collection
contains `mark`. Put this in `content/index.typ` and run `tola build` to see it on
the home page:

```typst
#import "@tola/icon:0.0.0": icon
#icon("brand:mark", module: "notes")
```

To replace the collection, put a full source record under
`[modules.notes.icons.collections.brand]` in the site's configuration. A replacement's
local path starts at the site root. To update a remote collection for this module,
run `tola update modules/notes/brand`.

### Use a local SVG

Save this file as `icons/brand/mark.svg` in the site root.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="currentColor" d="M12 2 22 22H2Z"/>
</svg>
```

Add the directory to `tola.toml`.

```toml
[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons/brand"
```

In a site created with `tola init`, put this in `content/index.typ`.

```typst
#import "@tola/icon:0.0.0": icon
#icon("brand:mark")
```

Run `tola dev` and open the printed URL. The home page contains an inline SVG. Its
`currentColor` follows the surrounding text. Use `icon-url` to publish a standalone SVG.

An ID has the form `namespace:name`, with case-sensitive names. Run `tola inspect icons brand`
to list the names in `brand`. Use `source-type = "local-json"` for a local IconifyJSON file.
Local SVGs must be self-contained. Give the root a positive `viewBox` size or positive
absolute dimensions. Explicit dimensions must use absolute units. Convert text to paths."#;

    pub(crate) fn local_paths(&self) -> impl Iterator<Item = &Path> {
        self.collections.values().filter_map(|source| match source {
            IconCollectionSource::LocalJson { path } => Some(path.as_path()),
            IconCollectionSource::LocalSvgDir { path } => Some(path.as_path()),
            IconCollectionSource::RemoteJson { .. } => None,
        })
    }

    pub(crate) fn validate_paths(&self, diagnostics: &mut ConfigDiagnostics) {
        for (namespace, source) in &self.collections {
            let path = match source {
                IconCollectionSource::LocalJson { path } => path,
                IconCollectionSource::LocalSvgDir { path } => path,
                IconCollectionSource::RemoteJson { .. } => continue,
            };
            let key = format!("icons.collections.{namespace}.path");
            super::path::validate_site_relative_path(
                path,
                &key,
                Self::FIELDS.collections,
                diagnostics,
            );
            if super::path::enters_internal_directory(path) {
                diagnostics.error(
                    Self::FIELDS.collections,
                    format!(
                        "icon collection `{namespace}`: `path` `{}` is inside Tola's `.tola` directory",
                        super::path::declared_path(path)
                    ),
                );
            }
        }
    }

    pub(crate) fn validate_internal_boundaries(
        &self,
        root: &Path,
        diagnostics: &mut ConfigDiagnostics,
    ) {
        for (namespace, source) in &self.collections {
            let path = match source {
                IconCollectionSource::LocalJson { path } => path,
                IconCollectionSource::LocalSvgDir { path } => path,
                IconCollectionSource::RemoteJson { .. } => continue,
            };
            if super::path::reaches_internal_directory(root, path) {
                diagnostics.error(Self::FIELDS.collections, format!("icon collection `{namespace}`: `path` `{}` is inside Tola's `.tola` directory", tola_diagnostic::display_path(path, root)));
            }
        }
    }

    pub(crate) fn normalize(&mut self, root: &Path) {
        for source in self.collections.values_mut() {
            let path = match source {
                IconCollectionSource::LocalJson { path } => path,
                IconCollectionSource::LocalSvgDir { path } => path,
                IconCollectionSource::RemoteJson { .. } => continue,
            };
            *path = root.join(&*path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_invalid_reports_help() {
        let err = toml::from_str::<IconsConfig>(
            r#"
            [collections.ui]
            source-type = "remote-json"
            url = "https://example.test/icons.json"
            sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            "#,
        )
        .unwrap_err();
        let reason = err.message();
        assert!(reason.contains("sha256"), "{reason}");
        assert!(reason.contains("tola.lock"), "{reason}");
        assert!(reason.contains("tola update"), "{reason}");
    }

    #[test]
    fn source_debug_redacts_queries() {
        let src = IconCollectionSource::RemoteJson {
            preset: None,
            version: None,
            url: Some("https://example.test/icons.json?token=private".into()),
            mirrors: None,
        };
        let dbg = format!("{src:?}");
        assert!(dbg.contains("<redacted>"));
        assert!(!dbg.contains("private"));
    }
}
