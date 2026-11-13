//! Icon collection sources available to `@tola/icon` during Bundle compilation.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tola_config::Config;

use crate::config::ConfigDiagnostics;
use crate::icon::iconify;

/// Import `icon` from `@tola/icon:0.0.0`; `icon("namespace:name")` inserts an SVG in HTML.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config, PartialEq, Eq)]
#[serde(default)]
#[config(section = "icons")]
pub struct IconsConfig {
    /// Each namespace names where its icons come from: a local SVG tree or IconifyJSON file, or
    /// a remote collection named by an npm-published preset or by a URL.
    pub collections: BTreeMap<String, IconCollectionSource>,
}

/// Where one icon namespace's content comes from.
#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "source-type", rename_all = "kebab-case")]
pub enum IconCollectionSource {
    /// One local IconifyJSON collection file.
    LocalJson { path: PathBuf },
    /// A local SVG tree; relative path components become hyphenated icon names.
    LocalSvgDir { path: PathBuf },
    /// IconifyJSON fetched over HTTP, named either by an npm-published preset or by an address
    /// the author writes.
    RemoteJson {
        /// The `@iconify-json` package that publishes this collection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preset: Option<String>,
        /// The package release to resolve, or the registry's latest release when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// The address serving this collection's IconifyJSON.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Addresses tried before the declared one, in the order written. Only a `preset` fills
        /// `{name}` and `{version}`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mirrors: Option<Vec<String>>,
    },
}

impl<'de> Deserialize<'de> for IconCollectionSource {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "source-type", rename_all = "kebab-case", deny_unknown_fields)]
        enum Written {
            LocalJson {
                path: PathBuf,
            },
            LocalSvgDir {
                path: PathBuf,
            },
            RemoteJson {
                preset: Option<String>,
                version: Option<String>,
                url: Option<String>,
                /// A key this model replaced; accepted only to name the repair.
                sha256: Option<String>,
                mirrors: Option<Vec<String>>,
            },
        }
        match Written::deserialize(deserializer)? {
            Written::LocalJson { path } => Ok(Self::LocalJson { path }),
            Written::LocalSvgDir { path } => Ok(Self::LocalSvgDir { path }),
            Written::RemoteJson {
                sha256: Some(_), ..
            } => Err(serde::de::Error::custom(
                "`sha256` is no longer part of `[icons.collections]`: icon integrity lives in `tola.lock`; delete `sha256` and run `tola update`",
            )),
            Written::RemoteJson {
                preset,
                version,
                url,
                mirrors,
                ..
            } => Ok(Self::RemoteJson {
                preset,
                version,
                url,
                mirrors,
            }),
        }
    }
}

impl fmt::Debug for IconCollectionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalJson { path } => formatter
                .debug_struct("LocalJson")
                .field("path", path)
                .finish(),
            Self::LocalSvgDir { path } => formatter
                .debug_struct("LocalSvgDir")
                .field("path", path)
                .finish(),
            Self::RemoteJson {
                preset,
                version,
                url,
                mirrors,
            } => {
                // A URL's query may hold credentials or a token, so it never reaches a log.
                let mut entry = formatter.debug_struct("RemoteJson");
                if let Some(preset) = preset {
                    entry.field("preset", preset);
                }
                if let Some(version) = version {
                    entry.field("version", version);
                }
                if url.is_some() {
                    entry.field("url", &"<redacted>");
                }
                if mirrors.is_some() {
                    entry.field("mirrors", &"<redacted>");
                }
                entry.finish()
            }
        }
    }
}

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
            if let IconCollectionSource::RemoteJson {
                preset,
                version,
                url,
                mirrors,
            } = source
            {
                validate_remote_json(
                    namespace,
                    preset.as_deref(),
                    version.as_deref(),
                    url.as_deref(),
                    mirrors.as_deref(),
                    diagnostics,
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

/// Accept exactly one way of naming a remote collection.
fn validate_remote_json(
    namespace: &str,
    preset: Option<&str>,
    version: Option<&str>,
    url: Option<&str>,
    mirrors: Option<&[String]>,
    diagnostics: &mut ConfigDiagnostics,
) {
    let field = IconsConfig::FIELDS.collections;
    match (preset, url) {
        (Some(preset), None) => {
            if !iconify::usable_preset(preset) {
                diagnostics.error_with_help(
                    field,
                    format!("icon collection `{namespace}`: `preset` `{preset}` is not an `@iconify-json` package name"),
                    "Write the package name after the `@iconify-json/` scope, for example `lucide`",
                );
                return;
            }
            if let Some(version) = version
                && !iconify::usable_version(version)
            {
                diagnostics.error_with_help(
                    field,
                    format!("icon collection `{namespace}`: `version` `{version}` is not a published release name"),
                    "Write the `@iconify-json` release exactly as the registry publishes it, or omit `version` to follow the latest release",
                );
            }
            if let Some(mirrors) = mirrors {
                validate_mirrors(namespace, mirrors, true, diagnostics);
            }
        }
        (None, Some(url)) => {
            if version.is_some() {
                version_needs_preset(namespace, diagnostics);
                return;
            }
            if !usable_url(url) {
                diagnostics.error_with_help(
                    field,
                    format!("icon collection `{namespace}`: `url` `{url}` is not a usable URL"),
                    "Use an absolute `http` or `https` URL with no credentials or fragment",
                );
            }
            if let Some(mirrors) = mirrors {
                validate_mirrors(namespace, mirrors, false, diagnostics);
            }
        }
        (Some(_), Some(_)) => diagnostics.error_with_help(
            field,
            format!("icon collection `{namespace}`: declare either `preset` or `url`, not both"),
            "remove the declaration this namespace does not need",
        ),
        (None, None) if version.is_some() => version_needs_preset(namespace, diagnostics),
        (None, None) => {
            // A namespace that is itself a usable preset name is almost always an author who
            // wrote the section header and stopped there.
            let help = if iconify::usable_preset(namespace) {
                format!(
                    "write `preset = \"{namespace}\"` to download the `@iconify-json/{namespace}` package"
                )
            } else {
                "write `preset` naming an `@iconify-json` package, or `url` naming an IconifyJSON file"
                    .to_owned()
            };
            diagnostics.error_with_help(
                field,
                format!("icon collection `{namespace}` declares no source"),
                help,
            );
        }
    }
}

/// `version` pins the release of a `preset`, so it needs one.
fn version_needs_preset(namespace: &str, diagnostics: &mut ConfigDiagnostics) {
    diagnostics.error_with_help(
        IconsConfig::FIELDS.collections,
        format!("icon collection `{namespace}`: `version` needs the `preset` it names"),
        "remove `version`, or write the `preset` whose release it is",
    );
}

/// Whether one address names an absolute `http` or `https` URL Tola may fetch.
fn usable_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && url.host_str().is_some()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

/// The placeholders a `mirror` address may carry, and nothing else.
const MIRROR_PLACEHOLDERS: [&str; 2] = ["name", "version"];

/// Whether every `{...}` in a mirror address names a placeholder Tola fills.
fn mirror_placeholders_are_known(mirror: &str) -> bool {
    let mut rest = mirror;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            return false;
        };
        if !MIRROR_PLACEHOLDERS.contains(&&rest[open + 1..open + close]) {
            return false;
        }
        rest = &rest[open + close + 1..];
    }
    !rest.contains('}')
}

/// Accept every mirror address of one remote collection.
///
/// A preset fills `{name}` and `{version}`; a declared `url` has nothing to fill, so its mirrors
/// must already be complete addresses.
fn validate_mirrors(
    namespace: &str,
    mirrors: &[String],
    placeholders: bool,
    diagnostics: &mut ConfigDiagnostics,
) {
    let field = IconsConfig::FIELDS.collections;
    for mirror in mirrors {
        if !placeholders {
            if mirror.contains('{') || mirror.contains('}') {
                diagnostics.error_with_help(
                    field,
                    format!(
                        "icon collection `{namespace}`: `mirrors` needs a `preset` to fill `{mirror}`"
                    ),
                    "write the address out, or declare a `preset`",
                );
                continue;
            }
        } else if !mirror_placeholders_are_known(mirror) {
            diagnostics.error_with_help(
                field,
                format!(
                    "icon collection `{namespace}`: `mirrors` has an unknown placeholder in `{mirror}`"
                ),
                "use `{name}` or `{version}`",
            );
            continue;
        }
        // The preset fills the placeholders after this check, so a stand-in answers for them here.
        let sample = mirror
            .replace("{name}", "icons")
            .replace("{version}", "1.0.0");
        if !usable_url(&sample) {
            diagnostics.error_with_help(
                field,
                format!("icon collection `{namespace}`: `mirrors` `{mirror}` is not a usable URL"),
                "use an absolute `http` or `https` URL without credentials or a fragment",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_refused_with_its_repair() {
        let error = toml::from_str::<IconsConfig>(
            r#"
            [collections.ui]
            source-type = "remote-json"
            url = "https://example.test/icons.json"
            sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            "#,
        )
        .unwrap_err();
        let reason = error.message();
        assert!(reason.contains("sha256"), "{reason}");
        assert!(reason.contains("tola.lock"), "{reason}");
        assert!(reason.contains("tola update"), "{reason}");
    }

    #[test]
    fn source_debug_redacts_url_queries() {
        let source = IconCollectionSource::RemoteJson {
            preset: None,
            version: None,
            url: Some("https://example.test/icons.json?token=private".into()),
            mirrors: None,
        };
        let debug = format!("{source:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("private"));
    }
}
