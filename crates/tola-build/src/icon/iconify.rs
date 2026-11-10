//! The npm registry releases that serve Iconify collections.
//!
//! A preset names an `@iconify-json/<preset>` package. The registry publishes each release's
//! tarball and its subresource integrity, so a preset resolves to bytes the registry vouches for;
//! the lock then records the digest of the IconifyJSON file those bytes carry.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;

use serde::Deserialize;
use sha2::{Digest, Sha512};

use crate::cancellation::BuildCancellation;
use crate::resources::BuildResources;

use super::IconError;
use super::input::MAX_COLLECTION_BYTES;

/// The registry every preset resolves through.
pub(super) const REGISTRY: &str = "https://registry.npmjs.org";

/// Registry metadata stays far below this; a hostile response cannot grow the resolution.
const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;

/// A package tarball carries the collection file plus a little packaging metadata.
const MAX_TARBALL_BYTES: usize = 64 * 1024 * 1024;

/// A package archive carries a handful of files, not thousands.
const MAX_TARBALL_ENTRIES: usize = 1024;

/// The collection file a package tarball carries.
const COLLECTION_FILE: &str = "icons.json";

/// Whether `preset` names an `@iconify-json` package.
pub(crate) fn usable_preset(preset: &str) -> bool {
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
pub(crate) fn usable_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 128
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+' | b'_'))
}

/// One published release of one preset.
#[derive(Debug, Clone)]
pub(super) struct Release {
    pub(super) preset: String,
    pub(super) version: String,
    /// The registry's canonical tarball address, the source the lock records.
    pub(super) tarball: String,
    integrity: Integrity,
}

/// Resolve one preset to the release the registry publishes.
pub(super) fn release(
    namespace: &str,
    preset: &str,
    version: Option<&str>,
    resources: &BuildResources,
    cancellation: &BuildCancellation,
) -> Result<Release, IconError> {
    let url = format!("{REGISTRY}/@iconify-json/{preset}");
    let bytes = resources
        .download(&url, MAX_METADATA_BYTES, cancellation)
        .map_err(|source| registry_failure(namespace, preset, &url, source))?;
    let package: Package =
        serde_json::from_slice(&bytes).map_err(|_| IconError::RegistryFormat {
            preset: preset.into(),
        })?;
    let version =
        match version {
            Some(version) => {
                if !package.versions.contains_key(version) {
                    return Err(IconError::RegistryRelease {
                        preset: preset.into(),
                        version: version.into(),
                        latest: package.dist_tags.get("latest").cloned(),
                    });
                }
                version.to_owned()
            }
            None => package.dist_tags.get("latest").cloned().ok_or_else(|| {
                IconError::RegistryFormat {
                    preset: preset.into(),
                }
            })?,
        };
    let published = package
        .versions
        .get(&version)
        .ok_or_else(|| IconError::RegistryFormat {
            preset: preset.into(),
        })?;
    let integrity = published
        .dist
        .integrity
        .as_deref()
        .and_then(Integrity::parse)
        .ok_or_else(|| IconError::RegistryIntegrity {
            preset: preset.into(),
            version: version.clone(),
        })?;
    Ok(Release {
        preset: preset.into(),
        version,
        tarball: published.dist.tarball.clone(),
        integrity,
    })
}

impl Release {
    /// Check one downloaded tarball against the integrity the registry published.
    pub(super) fn verify(&self, namespace: &str, tarball: &[u8]) -> Result<(), IconError> {
        if self.integrity.matches(tarball) {
            return Ok(());
        }
        Err(IconError::TarballIntegrity {
            namespace: namespace.into(),
            preset: self.preset.clone(),
            version: self.version.clone(),
            expected: self.integrity.to_string(),
            received: Integrity::of(tarball).to_string(),
        })
    }

    /// Take the IconifyJSON file one verified package tarball carries.
    pub(super) fn collection_json(
        &self,
        namespace: &str,
        tarball: &[u8],
    ) -> Result<Vec<u8>, IconError> {
        collection_json(namespace, &self.preset, &self.version, tarball)
    }
}

/// Take the IconifyJSON file one package tarball carries.
pub(super) fn collection_json(
    namespace: &str,
    preset: &str,
    version: &str,
    tarball: &[u8],
) -> Result<Vec<u8>, IconError> {
    let unreadable = || IconError::Tarball {
        namespace: namespace.into(),
        preset: preset.into(),
        version: version.into(),
    };
    let too_large = || IconError::TarballCollectionTooLarge {
        namespace: namespace.into(),
        preset: preset.into(),
        version: version.into(),
        limit: MAX_COLLECTION_BYTES,
    };
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    let entries = archive.entries().map_err(|_| unreadable())?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_TARBALL_ENTRIES {
            return Err(unreadable());
        }
        let entry = entry.map_err(|_| unreadable())?;
        let path = entry.path().map_err(|_| unreadable())?.to_path_buf();
        if path.file_name().and_then(|name| name.to_str()) != Some(COLLECTION_FILE) {
            continue;
        }
        if !entry.header().entry_type().is_file() || entry.size() > MAX_COLLECTION_BYTES as u64 {
            return Err(too_large());
        }
        let mut json = Vec::new();
        entry
            .take(MAX_COLLECTION_BYTES as u64 + 1)
            .read_to_end(&mut json)
            .map_err(|_| unreadable())?;
        if json.len() > MAX_COLLECTION_BYTES {
            return Err(too_large());
        }
        return Ok(json);
    }
    Err(IconError::TarballCollection {
        namespace: namespace.into(),
        preset: preset.into(),
        version: version.into(),
    })
}

/// How many bytes one tarball download may carry.
pub(super) const fn tarball_limit() -> usize {
    MAX_TARBALL_BYTES
}

/// Map one download failure onto the registry step that asked for it.
fn registry_failure(
    namespace: &str,
    preset: &str,
    url: &str,
    source: crate::resources::HttpDownloadError,
) -> IconError {
    use crate::resources::HttpDownloadError;
    match source {
        HttpDownloadError::NetworkDenied => IconError::NetworkDenied {
            namespace: namespace.into(),
            url: url.into(),
        },
        HttpDownloadError::Cancelled(cancelled) => IconError::Cancelled(cancelled),
        HttpDownloadError::Runtime(source) => IconError::NetworkRuntime(source),
        HttpDownloadError::WorkerStopped => IconError::NetworkWorker,
        HttpDownloadError::TooLarge { limit } => IconError::RemoteTooLarge {
            namespace: namespace.into(),
            url: url.into(),
            limit,
        },
        source @ HttpDownloadError::Request(_) => IconError::RegistryMetadata {
            namespace: namespace.into(),
            preset: preset.into(),
            url: url.into(),
            source,
        },
    }
}

/// The registry's document for one package.
#[derive(Debug, Deserialize)]
struct Package {
    #[serde(default, rename = "dist-tags")]
    dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    versions: BTreeMap<String, Published>,
}

/// One release in the registry's document.
#[derive(Debug, Deserialize)]
struct Published {
    dist: Dist,
}

/// The distribution facts the registry publishes for one release.
#[derive(Debug, Deserialize)]
struct Dist {
    tarball: String,
    #[serde(default)]
    integrity: Option<String>,
}

/// One published subresource integrity, restricted to the SHA-512 form Tola verifies.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Integrity(Vec<u8>);

impl Integrity {
    fn parse(encoded: &str) -> Option<Self> {
        let (algorithm, digest) = encoded.split_once('-')?;
        // The registry may publish several algorithms; only the strongest one is accepted.
        if algorithm != "sha512" {
            return None;
        }
        decode_base64(digest).map(Self)
    }

    fn of(bytes: &[u8]) -> Self {
        Self(Sha512::digest(bytes).to_vec())
    }

    fn matches(&self, bytes: &[u8]) -> bool {
        Sha512::digest(bytes).as_slice() == self.0
    }
}

impl fmt::Display for Integrity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sha512-")?;
        formatter.write_str(&encode_base64(&self.0))
    }
}

/// The standard base64 alphabet subresource integrity uses.
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(encoded.len() / 4 * 3);
    let mut accumulator = 0_u32;
    let mut bits = 0_u32;
    for byte in encoded.bytes() {
        if byte == b'=' {
            continue;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    Some(decoded)
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        encoded.push(BASE64[((triple >> 18) & 0x3f) as usize] as char);
        encoded.push(BASE64[((triple >> 12) & 0x3f) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            BASE64[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            BASE64[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}
