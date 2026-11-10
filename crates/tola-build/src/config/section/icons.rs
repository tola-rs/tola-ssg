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
    pub const HELP: &'static str = "\
Each entry under `[icons.collections]` is one namespace, and the namespace is the first half of
every `\"namespace:name\"` id you write in Typst: `brand:mark` names `mark` in the namespace
`brand`. Ids are case-sensitive. `icon()` checks the id when called, including when its returned
content is unused. A missing name may suggest a close match from that configured collection.

A directory of SVG files you own, and a collection published on the npm registry:

```toml
# SVG files you own: icons/brand/mark.svg becomes brand:mark.
[icons.collections.brand]
source-type = \"local-svg-dir\"     # or \"local-json\" for one IconifyJSON file
path = \"icons/brand\"

[icons.collections.lucide]
source-type = \"remote-json\"
preset = \"lucide\"                  # the newest release of @iconify-json/lucide
```

A preset names an `@iconify-json` package, such as `lucide`, `mdi`, `tabler`, or `simple-icons`. Tola
resolves it through the npm registry: it reads the release's `dist.integrity`, downloads that
tarball, verifies the bytes it received, and takes the collection file the tarball carries. Write
`version = \"0.468.0\"` beside the preset to pin the release instead of following the registry's
latest; that version is Iconify's packaging number for the collection, not the icon set's own
release.

Resolution happens when you build, run `tola dev`, or run `tola update`; its result goes to
`tola.lock`: the resolved version, the tarball address, and the SHA-256 of the collection bytes.
The lock, not the configuration, holds integrity. `tola vendor` freezes the locked bytes into
`vendor/icons/<namespace>.json`, and a pure build reads them from there.

`mirrors` lists other addresses tried when the registry's tarball cannot be downloaded, in the
order written. `{name}` and `{version}` come from the preset:

```toml
[icons.collections.lucide]
source-type = \"remote-json\"
preset = \"lucide\"
mirrors = [\"https://registry.npmmirror.com/@iconify-json/{name}/-/{name}-{version}.tgz\"]
```

A mirror must serve the same tarball the registry published, because that release's integrity pins
its bytes.

Write `url` when no registry package carries the collection you want:

```toml
[icons.collections.brand-remote]
source-type = \"remote-json\"
url = \"https://cdn.example.com/brand/1.0.0/icons.json\"
```

The two remote forms are exclusive: `version` belongs with `preset`. A `url` collection has no
published integrity, so its first download is trusted and recorded in `tola.lock`; every later
build verifies the same bytes. A `mirrors` list beside a `url` must write each address out.

Draw a configured icon in a page, and publish one as a file of its own:

```typst
#import \"@tola/icon:0.0.0\": icon, icon-url
#document(\"index.html\")[
  #icon(\"lucide:rocket\")
  #html.img(src: icon-url(\"brand:mark\"), alt: \"Brand mark\")
]
```

Export your own icons as self-contained SVG: convert text to paths, bring external symbols into
this file, and keep references within it. Give the root a `viewBox` with positive width and height,
or positive `width` and `height` in absolute units. Any explicit dimensions must use absolute
units, even with a `viewBox`; percentages and `em` are refused. For example, `icons/brand/mark.svg`
can contain:

```svg
<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\">
  <path fill=\"currentColor\" d=\"M12 2 22 22H2Z\"/>
</svg>
```

An icon keeps the colors its own SVG draws. Artwork painted with `currentColor` follows the `color`
of the page around it, so a theme or a CSS class recolors it; artwork with fixed fills and strokes
keeps them whatever the page does. Every `lucide` icon is painted with `currentColor`, which is why
that collection suits a themed site. A published file such as `icon-url(\"brand:mark\")` inherits
nothing, so reach for `icon()` when the artwork should follow the theme.

See what a site can use:

```sh
tola inspect icons                       # every namespace, its source, and its icon count
tola inspect icons lucide                # one namespace/name row per icon or alias
tola inspect icons lucide --interactive  # browse those names in a table
```

Without a namespace, the JSON array has one row per collection: its namespace, resolved source,
own icon prefix, and icon count. Naming a namespace returns one `namespace`/`name` row per icon
or alias, in lexical order. Interactive selection returns the selected row.

`tola inspect icons` reads the locked inputs; run `tola update` first when a remote collection has
not been resolved yet. `tola vendor` freezes a site's remote collections into its own
`icons/<namespace>.json`, so another machine builds offline.";

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
