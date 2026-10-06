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
    /// a remote collection named by an indexed preset or by a URL and digest.
    pub collections: BTreeMap<String, IconCollectionSource>,
}

/// Where one icon namespace's content comes from.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "source-type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum IconCollectionSource {
    /// One local IconifyJSON collection file.
    LocalJson { path: PathBuf },
    /// A local SVG tree; relative path components become hyphenated icon names.
    LocalSvgDir { path: PathBuf },
    /// IconifyJSON fetched over HTTP, named either by an indexed collection or by bytes the author
    /// pins themselves.
    RemoteJson {
        /// The Iconify collection Tola indexes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preset: Option<String>,
        /// The release to fetch, or the newest release Tola indexes when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// The URL serving this collection's IconifyJSON.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// The SHA-256 digest of the bytes `url` serves.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<Sha256Digest>,
    },
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
                sha256,
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
                if let Some(sha256) = sha256 {
                    entry.field("sha256", sha256);
                }
                entry.finish()
            }
        }
    }
}

/// The expected SHA-256 digest of a remote collection's exact bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub(crate) fn bytes(self) -> [u8; 32] {
        self.0
    }

    /// Decode the digest of one indexed release.
    ///
    /// A malformed digest in the generated index fails the compilation.
    pub(crate) const fn from_hex(encoded: &str) -> Self {
        match decode_hex(encoded) {
            Some(bytes) => Self(bytes),
            None => panic!("an indexed icon collection digest must be 64 hexadecimal digits"),
        }
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = &'static str;

    fn try_from(encoded: String) -> Result<Self, Self::Error> {
        decode_hex(&encoded)
            .map(Self)
            .ok_or("`sha256` is not 64 hexadecimal digits")
    }
}

impl From<Sha256Digest> for String {
    fn from(digest: Sha256Digest) -> Self {
        encode_hex(digest.0)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&encode_hex(self.0))
    }
}

/// Decode the 64 hexadecimal digits of one digest.
const fn decode_hex(encoded: &str) -> Option<[u8; 32]> {
    let digits = encoded.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    let mut index = 0;
    while index < bytes.len() {
        let high = match hex_digit(digits[index * 2]) {
            Some(high) => high,
            None => return None,
        };
        let low = match hex_digit(digits[index * 2 + 1]) {
            Some(low) => low,
            None => return None,
        };
        bytes[index] = (high << 4) | low;
        index += 1;
    }
    Some(bytes)
}

const fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
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
                sha256,
            } = source
            {
                validate_remote_json(
                    namespace,
                    preset.as_deref(),
                    version.as_deref(),
                    url.as_deref(),
                    *sha256,
                    diagnostics,
                );
            }
        }
    }

    /// What `tola help "[icons]"` adds under its table.
    pub const HELP: &'static str = "\
Each entry under `[icons.collections]` is one namespace, and the namespace is the first half of
every `\"namespace:name\"` id you write in Typst: `brand:mark` names `mark` in the namespace
`brand`. Ids are case-sensitive.

A directory of SVG files you own, and a collection Tola already indexes:

```toml
# SVG files you own: icons/brand/mark.svg becomes brand:mark.
[icons.collections.brand]
source-type = \"local-svg-dir\"     # or \"local-json\" for one IconifyJSON file
path = \"icons/brand\"

[icons.collections.lucide]
source-type = \"remote-json\"
preset = \"lucide\"                  # newest release Tola indexes
```

A preset names a collection Tola already indexes, such as `lucide`, `mdi`, `tabler`, or
`simple-icons`, so `preset = \"lucide\"` is all you write: Tola supplies the download address and the
digest of the bytes it serves. Adding `version = \"1.2.136\"` beside the preset holds that release
still, so a build on another day fetches the same icons.

Write `url` with the `sha256` of its bytes instead when the collection is one Tola does not index,
or a release it does not pin:

```toml
[icons.collections.brand-remote]
source-type = \"remote-json\"
url = \"https://cdn.example.com/brand/1.0.0/icons.json\"
sha256 = \"<64 hexadecimal digits>\"
```

The two remote forms are exclusive: `version` belongs with `preset`, `sha256` with `url`. To find
the digest, download that exact file, run `shasum -a 256 icons.json`, and paste the 64 hexadecimal
digits it prints; a build refuses bytes that do not match them.

Draw a configured icon in a page, and publish one as a file of its own:

```typst
#import \"@tola/icon:0.0.0\": icon, icon-url
#document(\"index.html\")[
  #icon(\"lucide:rocket\")
  #html.img(src: icon-url(\"brand:mark\"), alt: \"Brand mark\")
]
```

An icon keeps the colors its own SVG draws. Artwork painted with `currentColor` follows the `color`
of the page around it, so a theme or a CSS class recolors it; artwork with fixed fills and strokes
keeps them whatever the page does. Every `lucide` icon is painted with `currentColor`, which is why
that collection suits a themed site. A published file such as `icon-url(\"brand:mark\")` inherits
nothing, so reach for `icon()` when the artwork should follow the theme.

See what a site can use:

```sh
tola inspect icons                       # every namespace, its source, and its icon count
tola inspect icons lucide                # adds that namespace's icon names
tola inspect icons lucide --interactive  # browse those names in a table
```

The command prints a JSON array with one row per namespace: its resolved source, the collection's
own icon prefix, and its icon count; naming a namespace adds that collection's `names`.

A remote collection downloads and caches exactly as a build downloads it. `tola vendor` freezes a
site's remote collections into its own `icons/<namespace>.json`, so another machine builds offline.";

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
                diagnostics.error(Self::FIELDS.collections, format!("icon collection `{namespace}`: `path` `{}` is inside Tola's `.tola` directory", crate::filesystem::display_path(path, root)));
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

/// Accept exactly one way of naming a remote collection, and only a collection Tola indexes.
fn validate_remote_json(
    namespace: &str,
    preset: Option<&str>,
    version: Option<&str>,
    url: Option<&str>,
    sha256: Option<Sha256Digest>,
    diagnostics: &mut ConfigDiagnostics,
) {
    let field = IconsConfig::FIELDS.collections;
    match (preset, url) {
        (Some(preset), None) => {
            if sha256.is_some() {
                diagnostics.error_with_help(
                    field,
                    format!("icon collection `{namespace}`: `sha256` needs the `url` it verifies"),
                    "remove `sha256`, or write the `url` it belongs to",
                );
                return;
            }
            let Some(indexed) = iconify::indexed(preset) else {
                diagnostics.error_with_help(
                    field,
                    format!(
                        "icon collection `{namespace}`: Tola does not index an Iconify collection named `{preset}`"
                    ),
                    "check the name, or declare the collection's `url` with the `sha256` of its bytes",
                );
                return;
            };
            if let Some(version) = version
                && indexed.release(version).is_none()
            {
                diagnostics.error_with_help(
                    field,
                    format!(
                        "icon collection `{namespace}`: Tola does not index release `{version}` of `{preset}`"
                    ),
                    format!(
                        "use {}, or declare that release's `url` with the `sha256` of its bytes",
                        indexed_versions(indexed)
                    ),
                );
            }
        }
        (None, Some(url)) => {
            if version.is_some() {
                diagnostics.error_with_help(
                    field,
                    format!("icon collection `{namespace}`: `version` needs the `preset` it names"),
                    "remove `version`, or write the `preset` whose release it is",
                );
                return;
            }
            if sha256.is_none() {
                diagnostics.error_with_help(
                    field,
                    format!(
                        "icon collection `{namespace}`: `url` needs the `sha256` of the bytes it serves"
                    ),
                    "download that file and run `shasum -a 256` on it, then write the digest",
                );
                return;
            }
            let valid = url::Url::parse(url).is_ok_and(|url| {
                matches!(url.scheme(), "https" | "http")
                    && url.host_str().is_some()
                    && url.fragment().is_none()
                    && url.username().is_empty()
                    && url.password().is_none()
            });
            if !valid {
                diagnostics.error_with_help(
                    field,
                    format!("icon collection `{namespace}`: `url` `{url}` is not a usable URL"),
                    "Use an absolute `http` or `https` URL with no credentials or fragment",
                );
            }
        }
        (Some(_), Some(_)) => diagnostics.error_with_help(
            field,
            format!("icon collection `{namespace}`: declare either `preset` or `url`, not both"),
            "remove the declaration this namespace does not need",
        ),
        (None, None) if version.is_some() => diagnostics.error_with_help(
            field,
            format!("icon collection `{namespace}`: `version` needs the `preset` it names"),
            "remove `version`, or write the `preset` whose release it is",
        ),
        (None, None) if sha256.is_some() => diagnostics.error_with_help(
            field,
            format!("icon collection `{namespace}`: `sha256` needs the `url` it verifies"),
            "remove `sha256`, or write the `url` it belongs to",
        ),
        (None, None) => diagnostics.error_with_help(
            field,
            format!("icon collection `{namespace}` declares no source"),
            "write `preset = \"…\"` for a collection Tola indexes, or `url` with `sha256`",
        ),
    }
}

/// The releases Tola indexes for one collection, bounded so one diagnostic stays readable.
fn indexed_versions(indexed: &iconify::IndexedCollection) -> String {
    let named = indexed
        .versions()
        .take(3)
        .map(|version| format!("`{version}`"))
        .collect::<Vec<_>>()
        .join(", ");
    match indexed.versions().count().saturating_sub(3) {
        0 => named,
        remaining => format!("{named}, or {remaining} more"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn icon_sources_group_by_namespace() {
        let config: IconsConfig = toml::from_str(
            r#"
            [collections.brand]
            source-type = "local-svg-dir"
            path = "assets/brand"
            [collections.ui]
            source-type = "local-json"
            path = "icons/ui.json"
            [collections.social]
            source-type = "remote-json"
            url = "https://example.test/social/1.0.0/icons.json"
            sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            [collections.icons]
            source-type = "remote-json"
            preset = "lucide"
            "#,
        )
        .unwrap();
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        config.validate_paths(&mut diagnostics);
        assert!(diagnostics.into_result().is_ok());
        assert_eq!(config.collections.len(), 4);
        assert_eq!(
            config.local_paths().collect::<Vec<_>>(),
            [Path::new("assets/brand"), Path::new("icons/ui.json")]
        );
    }

    #[test]
    fn remote_source_declares_one_origin() {
        let cases = [
            (
                r#"
                [collections.ui]
                source-type = "remote-json"
                "#,
                "declares no source",
            ),
            (
                r#"
                [collections.ui]
                source-type = "remote-json"
                url = "https://example.test/icons.json"
                "#,
                "needs the `sha256`",
            ),
            (
                r#"
                [collections.ui]
                source-type = "remote-json"
                url = "https://example.test/icons.json"
                sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                preset = "tabler"
                "#,
                "declare either `preset` or `url`, not both",
            ),
            (
                r#"
                [collections.ui]
                source-type = "remote-json"
                preset = "tabler"
                sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                "#,
                "`sha256` needs the `url`",
            ),
            (
                r#"
                [collections.ui]
                source-type = "remote-json"
                version = "1.2.3"
                "#,
                "`version` needs the `preset`",
            ),
            (
                r#"
                [collections.ui]
                source-type = "remote-json"
                url = "https://example.test/icons.json"
                sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                version = "1.2.3"
                "#,
                "`version` needs the `preset`",
            ),
        ];
        for (source, expected) in cases {
            let config: IconsConfig = toml::from_str(source).unwrap();
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate(&mut diagnostics);
            assert_eq!(diagnostics.errors().len(), 1, "{source}");
            assert!(
                diagnostics.errors()[0].message.contains(expected),
                "{source}: {}",
                diagnostics.errors()[0].message
            );
        }
    }

    #[test]
    fn unindexed_preset_or_release_is_refused() {
        let newest_lucide = iconify::indexed("lucide")
            .unwrap()
            .versions()
            .next()
            .unwrap();
        for (declaration, expected, help_fragment) in [
            (
                "preset = \"not-a-real-collection\"",
                "does not index an Iconify collection named `not-a-real-collection`",
                "check the name",
            ),
            (
                "preset = \"lucide\"\nversion = \"0.0.1\"",
                "does not index release `0.0.1` of `lucide`",
                newest_lucide,
            ),
        ] {
            let config: IconsConfig = toml::from_str(&format!(
                "[collections.ui]\nsource-type = \"remote-json\"\n{declaration}\n"
            ))
            .unwrap();
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate(&mut diagnostics);

            let errors = diagnostics.errors();
            assert_eq!(errors.len(), 1, "{declaration}");
            assert!(
                errors[0].message.contains(expected),
                "{}",
                errors[0].message
            );
            let help = errors[0]
                .help
                .as_deref()
                .expect("the refusal names what to write instead");
            assert!(help.contains(help_fragment), "{help}");
        }
    }

    #[test]
    fn local_source_refuses_remote_fields() {
        let error = toml::from_str::<IconsConfig>(
            r#"
            [collections.ui]
            source-type = "local-json"
            path = "icons/ui.json"
            url = "https://example.test/icons.json"
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn malformed_digest_is_refused_at_decode() {
        let error = toml::from_str::<IconsConfig>(
            r#"
            [collections.ui]
            source-type = "remote-json"
            url = "https://example.test/icons.json"
            sha256 = "abcd"
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("64 hexadecimal digits"));
    }

    #[test]
    fn digests_round_trip_their_written_form() {
        let digest = Sha256Digest::try_from(DIGEST.to_owned()).unwrap();
        assert_eq!(digest.to_string(), DIGEST);
        assert_eq!(String::from(digest), DIGEST);
        assert_eq!(Sha256Digest::from_hex(DIGEST), digest);
        assert_eq!(
            Sha256Digest::try_from(DIGEST.to_uppercase())
                .unwrap()
                .to_string(),
            DIGEST
        );
    }

    #[test]
    fn local_sources_normalize_once() {
        let mut config: IconsConfig = toml::from_str(
            r#"
            [collections.brand]
            source-type = "local-svg-dir"
            path = "brand"
            "#,
        )
        .unwrap();
        config.normalize(Path::new("/site"));
        assert_eq!(config.local_paths().next(), Some(Path::new("/site/brand")));
    }

    #[test]
    fn source_debug_redacts_url_queries() {
        let source = IconCollectionSource::RemoteJson {
            preset: None,
            version: None,
            url: Some("https://example.test/icons.json?token=private".into()),
            sha256: Some(Sha256Digest([0; 32])),
        };
        let debug = format!("{source:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("private"));
    }

    #[test]
    fn local_sources_stay_inside_the_site() {
        for path in ["", "../icons", ".tola/icons", "/external/icons"] {
            let config = IconsConfig {
                collections: BTreeMap::from([(
                    "brand".into(),
                    IconCollectionSource::LocalSvgDir { path: path.into() },
                )]),
            };
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate_paths(&mut diagnostics);
            assert!(!diagnostics.errors().is_empty(), "accepted {path}");
            assert!(
                diagnostics
                    .errors()
                    .iter()
                    .all(|diagnostic| diagnostic.field == IconsConfig::FIELDS.collections)
            );
        }
    }

    #[test]
    fn namespace_diagnostics_name_collection() {
        let config = IconsConfig {
            collections: BTreeMap::from([(
                "../brand".into(),
                IconCollectionSource::LocalSvgDir {
                    path: "brand".into(),
                },
            )]),
        };
        let mut diagnostics = ConfigDiagnostics::new();
        config.validate(&mut diagnostics);
        assert_eq!(diagnostics.errors().len(), 1);
        assert!(diagnostics.errors()[0].message.contains("../brand"));
    }
}
