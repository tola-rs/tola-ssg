//! Icon collection declarations shared by sites and modules.

use crate::fragment::{FragmentError, FragmentStep, read_fragment};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// Collections belonging to one site or module instance.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Icons {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalFields {
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteFields {
    preset: Option<String>,
    version: Option<String>,
    url: Option<String>,
    sha256: Option<String>,
    mirrors: Option<Vec<String>>,
}

const REMOVED_ICON_CHECKSUM: &str = "`sha256` is no longer part of `[icons.collections]`: icon integrity lives in `tola.lock`; delete `sha256` and run `tola update`";

impl<'de> Deserialize<'de> for IconCollectionSource {
    fn deserialize<D: serde::Deserializer<'de>>(deser: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "source-type", rename_all = "kebab-case")]
        enum Written {
            LocalJson(LocalFields),
            LocalSvgDir(LocalFields),
            RemoteJson(RemoteFields),
        }
        match Written::deserialize(deser)? {
            Written::LocalJson(LocalFields { path }) => Ok(Self::LocalJson { path }),
            Written::LocalSvgDir(LocalFields { path }) => Ok(Self::LocalSvgDir { path }),
            Written::RemoteJson(RemoteFields {
                sha256: Some(_), ..
            }) => Err(serde::de::Error::custom(REMOVED_ICON_CHECKSUM)),
            Written::RemoteJson(RemoteFields {
                preset,
                version,
                url,
                mirrors,
                ..
            }) => Ok(Self::RemoteJson {
                preset,
                version,
                url,
                mirrors,
            }),
        }
    }
}

impl fmt::Debug for IconCollectionSource {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalJson { path } => fmt.debug_struct("LocalJson").field("path", path).finish(),
            Self::LocalSvgDir { path } => {
                fmt.debug_struct("LocalSvgDir").field("path", path).finish()
            }
            Self::RemoteJson {
                preset,
                version,
                url,
                mirrors,
            } => {
                // Redact URLs and mirrors so query tokens never reach logs.
                let mut entry = fmt.debug_struct("RemoteJson");
                if let Some(preset) = preset {
                    entry.field("preset", preset);
                }
                if let Some(ver) = version {
                    entry.field("version", ver);
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

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct FragmentFields {
    source_type: Option<String>,
    path: Option<PathBuf>,
    preset: Option<String>,
    version: Option<String>,
    url: Option<String>,
    mirrors: Option<Vec<String>>,
    sha256: Option<String>,
}

impl Icons {
    /// Validate collection names and source selectors.
    pub fn validate(&self) -> Result<(), String> {
        for (name, src) in &self.collections {
            name.parse::<tola_icons::IconCollectionName>()
                .map_err(|err| format!("icon collection `{name}`: {err}"))?;
            src.validate()
                .map_err(|msg| format!("icon collection `{name}`: {msg}"))?;
        }
        Ok(())
    }

    /// Check supplied fields before configuration fragments fill required fields.
    pub fn validate_fragment(value: &toml::Value) -> Result<(), FragmentError> {
        #[derive(Deserialize)]
        struct Fields {
            collections: Option<BTreeMap<String, toml::Value>>,
        }
        let fields = read_fragment::<Fields>(value)?;
        for (name, val) in fields.collections.unwrap_or_default() {
            IconCollectionSource::validate_fragment(&val).map_err(|err| {
                err.at(FragmentStep::Key(name))
                    .at(FragmentStep::Key("collections".into()))
            })?;
        }
        Ok(())
    }
}

impl IconCollectionSource {
    /// Validate the source independently of its declaring directory.
    pub fn validate(&self) -> Result<(), String> {
        let Self::RemoteJson {
            preset,
            version,
            url,
            mirrors,
        } = self
        else {
            return Ok(());
        };
        match (preset.as_deref(), url.as_deref()) {
            (Some(preset), None) if usable_preset(preset) => {
                if version.as_deref().is_some_and(|ver| !usable_version(ver)) {
                    return Err("`version` must name an exact published release".into());
                }
            }
            (None, Some(url)) if usable_url(url) && version.is_none() => {}
            _ => {
                return Err(
                    "declare either a usable `preset` with optional `version`, or an absolute HTTP(S) `url`"
                        .into(),
                );
            }
        }
        for mirror in mirrors.iter().flatten() {
            let sample = if preset.is_some() {
                mirror
                    .replace("{name}", "icons")
                    .replace("{version}", "1.0.0")
            } else {
                mirror.clone()
            };
            if sample.contains(['{', '}']) || !usable_url(&sample) {
                return Err(
                    "`mirrors` must contain HTTP(S) URLs; a preset permits `{name}` and `{version}`"
                        .into(),
                );
            }
        }
        Ok(())
    }

    /// Check a partial source declaration without requiring its missing fields.
    pub fn validate_fragment(value: &toml::Value) -> Result<(), FragmentError> {
        let fields = read_fragment::<FragmentFields>(value)?;
        if fields.sha256.is_some() {
            return Err(FragmentError::invalid(REMOVED_ICON_CHECKSUM));
        }
        let has_remote = fields.preset.is_some()
            || fields.version.is_some()
            || fields.url.is_some()
            || fields.mirrors.is_some();
        match fields.source_type.as_deref() {
            Some("local-json" | "local-svg-dir") if !has_remote => Ok(()),
            Some("remote-json") if fields.path.is_none() => Ok(()),
            None if fields.path.is_none() || !has_remote => Ok(()),
            _ => Err(FragmentError::invalid(
                "use fields for one icon source: a local path or a remote collection",
            )),
        }
    }

    /// Local source path, when the collection is provided by a file or directory.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::LocalJson { path } | Self::LocalSvgDir { path } => Some(path),
            Self::RemoteJson { .. } => None,
        }
    }
}

fn usable_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

/// Whether `preset` names an `@iconify-json` package.
pub fn usable_preset(preset: &str) -> bool {
    !preset.is_empty()
        && preset.len() <= 128
        && preset.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
        && preset
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether `version` could name a published release.
pub fn usable_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 128
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+' | b'_'))
}
