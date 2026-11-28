//! `tola.lock`: resolved commit, version, and digest of every input a site selects.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::hash::Sha256Digest;

mod write;

/// Lock file name relative to the site root.
pub const LOCK_FILE: &str = "tola.lock";

/// Resolved inputs for a site, keyed by alias, package spec, or icon namespace.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct InputLock {
    /// Remote and local modules, keyed by alias.
    pub modules: BTreeMap<String, LockedModule>,
    /// Typst packages read by the compiler, keyed by `@namespace/name`.
    pub packages: BTreeMap<String, LockedPackage>,
    /// Remote icon collections, keyed by namespace.
    pub icons: BTreeMap<String, LockedIcon>,
}

impl InputLock {
    /// Lock file path under `site_root`.
    pub fn path(site_root: &Path) -> PathBuf {
        site_root.join(LOCK_FILE)
    }

    /// Read the site lock, returning `None` if no lock file exists yet.
    pub fn read(site_root: &Path) -> Result<Option<Self>, LockError> {
        let path = Self::path(site_root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(LockError::Read { path, source }),
        };
        Self::parse(&text).map(Some)
    }

    /// Parse a lock file, detecting legacy formats before reporting syntax errors.
    pub fn parse(text: &str) -> Result<Self, LockError> {
        let raw: RawLock = match toml::from_str(text) {
            Ok(raw) => raw,
            Err(source) => {
                if let Some(err) = legacy_error(text) {
                    return Err(err);
                }
                return Err(LockError::Parse { source });
            }
        };
        raw.into_lock()
    }

    /// Serialize the lock as pretty-printed TOML.
    pub fn to_toml(&self) -> Result<String, LockError> {
        toml::to_string_pretty(self).map_err(|source| LockError::Encode { source })
    }
}

/// Failure reading, parsing, or writing `tola.lock`.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("could not read `tola.lock`")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`tola.lock` is not a valid input lock")]
    Parse {
        #[source]
        source: toml::de::Error,
    },
    #[error("could not acquire the write lock for `tola.lock`")]
    Access(#[source] std::io::Error),
    #[error("`tola.lock` entry `{section}.{name}` changed during this command; run it again")]
    Conflict { section: &'static str, name: String },
    #[error(
        "`tola.lock` carries a format version; delete it and run `tola update` to resolve the site's inputs again"
    )]
    FormatVersion,
    #[error(
        "`tola.lock` predates the input model v2; delete it and run `tola update` to resolve the site's inputs again"
    )]
    Legacy,
    #[error("Tola could not encode `tola.lock`")]
    Encode {
        #[source]
        source: toml::ser::Error,
    },
    #[error("Tola could not stage `tola.lock` beside the site")]
    Stage {
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not write `tola.lock`")]
    Write {
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not flush `tola.lock`")]
    Flush {
        #[source]
        source: std::io::Error,
    },
    #[error("could not replace `tola.lock`")]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`tola.lock` records module `{alias}` without a repository")]
    ModuleRepository { alias: String },
    #[error("`tola.lock` records module `{alias}` without a commit")]
    ModuleCommit { alias: String },
    #[error("`tola.lock` records module `{alias}` with a commit that is not 40 hexadecimal digits")]
    ModuleCommitFormat { alias: String },
    #[error("`tola.lock` records module `{alias}` with a revision that is not a full commit")]
    ModuleRevisionFormat { alias: String },
    #[error("the `subdir` of module `{alias}` in `tola.lock` must stay inside the module")]
    ModuleSubdir { alias: String },
    #[error("module `{alias}` in `tola.lock` declares more than one source")]
    ModuleSourceAmbiguous { alias: String },
    #[error("`tola.lock` records module `{alias}` without `git`, `path`, or `tarball`")]
    ModuleSourceMissing { alias: String },
    #[error(
        "local module `{alias}` in `tola.lock` may not record `revision`, `commit`, or `subdir`"
    )]
    LocalModuleFields { alias: String },
    #[error("local module `{alias}` in `tola.lock` needs a `path` inside the site")]
    LocalModulePath { alias: String },
    #[error("`tola.lock` records module `{alias}` without `tree_sha256`")]
    ModuleTreeHash { alias: String },
    #[error(
        "`tola.lock` records module `{alias}` with a `tree_sha256` that is not 64 hexadecimal digits"
    )]
    ModuleTreeHashFormat { alias: String },
    #[error("`tola.lock` needs a valid `tarball_sha256` for module `{alias}`")]
    ModuleTarballHash { alias: String },
    #[error(
        "the tarball source of module `{alias}` in `tola.lock` needs a nonempty `url` or `path`"
    )]
    ModuleTarballLocation { alias: String },
    #[error("module `{alias}` in `tola.lock` has fields belonging to a different source type")]
    ModuleSourceFields { alias: String },
    #[error("`tola.lock` records package `{name}` without a version")]
    PackageVersion { name: String },
    #[error("`tola.lock` records package `{name}` without `tree_sha256`")]
    PackageTreeHash { name: String },
    #[error(
        "`tola.lock` records package `{name}` with a `tree_sha256` that is not 64 hexadecimal digits"
    )]
    PackageTreeHashFormat { name: String },
    #[error("`tola.lock` records icon collection `{name}` without a source")]
    IconSource { name: String },
    #[error("`tola.lock` records icon collection `{name}` without a URL")]
    IconUrl { name: String },
    #[error("`tola.lock` records icon collection `{name}` without `sha256`")]
    IconDigest { name: String },
    #[error(
        "`tola.lock` records icon collection `{name}` with a `sha256` that is not 64 hexadecimal digits"
    )]
    IconDigestFormat { name: String },
    #[error("`tola.lock` records icon collection `{name}` without `size`")]
    IconSize { name: String },
}

/// Resolved identity of one module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum LockedModule {
    /// Module fetched from a Git repository.
    Git {
        /// Repository URL or locator.
        git: String,
        /// Revision selector; `None` selects the default branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<LockedRevision>,
        /// Subdirectory inside the repository; empty for the repository root.
        #[serde(default, skip_serializing_if = "path_is_empty")]
        subdir: PathBuf,
        /// Resolved 40-digit commit hash.
        commit: String,
        /// Digest of the materialized module directory.
        tree_sha256: Sha256Digest,
        /// Private child modules locked under this module.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        modules: BTreeMap<String, LockedModule>,
    },
    /// Local module inside the site directory.
    Local {
        /// Module directory relative to the site root.
        path: PathBuf,
        /// Digest of the module directory.
        tree_sha256: Sha256Digest,
        /// Private child modules locked under this module.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        modules: BTreeMap<String, LockedModule>,
    },
    /// Downloaded or local module archive and its expanded contents.
    Tarball {
        tarball: TarballLocation,
        tarball_sha256: Sha256Digest,
        tree_sha256: Sha256Digest,
        /// Private child modules locked under this module.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        modules: BTreeMap<String, LockedModule>,
    },
}

/// Source location for a module tarball.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum TarballLocation {
    Url { url: String },
    File { path: PathBuf },
}

impl TarballLocation {
    /// Reject embedded credentials, fragments, non-HTTPS URLs, and empty paths.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Url { url } => {
                let parsed = url::Url::parse(url)
                    .map_err(|_| "`tarball.url` must be an HTTPS download address")?;
                if parsed.scheme() != "https"
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(
                        "`tarball.url` must use HTTPS without embedded credentials or a fragment",
                    );
                }
            }
            Self::File { path } if path.as_os_str().is_empty() => {
                return Err("`tarball.path` must not be empty");
            }
            Self::File { .. } => {}
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TarballLocation {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(
            deny_unknown_fields,
            expecting = "a table with exactly one of `url` or `path`"
        )]
        struct Fields {
            url: Option<String>,
            path: Option<PathBuf>,
        }
        let fields = Fields::deserialize(d)?;
        let loc = match (fields.url, fields.path) {
            (Some(url), None) => Self::Url { url },
            (None, Some(path)) => Self::File { path },
            _ => {
                return Err(serde::de::Error::custom(
                    "choose exactly one of `tarball.url` or `tarball.path`",
                ));
            }
        };
        loc.validate().map_err(serde::de::Error::custom)?;
        Ok(loc)
    }
}

impl LockedModule {
    /// Digest of the module directory read by this build.
    pub fn tree_sha256(&self) -> Sha256Digest {
        match self {
            Self::Git { tree_sha256, .. }
            | Self::Local { tree_sha256, .. }
            | Self::Tarball { tree_sha256, .. } => *tree_sha256,
        }
    }

    /// Private child modules locked under this module.
    pub fn children(&self) -> &BTreeMap<String, LockedModule> {
        match self {
            Self::Git { modules, .. }
            | Self::Local { modules, .. }
            | Self::Tarball { modules, .. } => modules,
        }
    }

    /// Mutable private child modules locked under this module.
    pub fn children_mut(&mut self) -> &mut BTreeMap<String, LockedModule> {
        match self {
            Self::Git { modules, .. }
            | Self::Local { modules, .. }
            | Self::Tarball { modules, .. } => modules,
        }
    }
}

/// Revision selector requested for a remote module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockedRevision {
    Tag(String),
    Branch(String),
    Rev(String),
}

/// Locked Typst package version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub version: String,
    pub tree_sha256: Sha256Digest,
}

/// Locked remote icon collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedIcon {
    pub source: IconSource,
    pub url: String,
    pub sha256: Sha256Digest,
    pub size: u64,
}

/// Declaration an icon collection resolved from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconSource {
    Preset { preset: String, version: String },
    Url { url: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLock {
    #[serde(default)]
    modules: BTreeMap<String, RawModule>,
    #[serde(default)]
    packages: BTreeMap<String, RawPackage>,
    #[serde(default)]
    icons: BTreeMap<String, RawIcon>,
}

impl RawLock {
    fn into_lock(self) -> Result<InputLock, LockError> {
        Ok(InputLock {
            modules: convert_map(self.modules, |alias, m| m.into_locked(alias))?,
            packages: convert_map(self.packages, |name, pkg| pkg.into_locked(name))?,
            icons: convert_map(self.icons, |name, icon| icon.into_locked(name))?,
        })
    }
}

fn convert_map<R, L>(
    raw: BTreeMap<String, R>,
    mut convert: impl FnMut(&str, R) -> Result<L, LockError>,
) -> Result<BTreeMap<String, L>, LockError> {
    raw.into_iter()
        .map(|(k, v)| {
            let locked = convert(&k, v)?;
            Ok((k, locked))
        })
        .collect()
}

fn parse_digest(
    raw: Option<String>,
    missing: impl FnOnce() -> LockError,
    invalid: impl FnOnce() -> LockError,
) -> Result<Sha256Digest, LockError> {
    let hex = raw.ok_or_else(missing)?;
    Sha256Digest::from_hex(&hex).ok_or_else(invalid)
}

fn require_nonempty(
    raw: Option<String>,
    err: impl FnOnce() -> LockError,
) -> Result<String, LockError> {
    raw.filter(|s| !s.is_empty()).ok_or_else(err)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModule {
    #[serde(default)]
    git: Option<String>,
    #[serde(default)]
    revision: Option<LockedRevision>,
    #[serde(default)]
    subdir: Option<PathBuf>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    tree_sha256: Option<String>,
    tarball: Option<TarballLocation>,
    tarball_sha256: Option<String>,
    #[serde(default)]
    modules: BTreeMap<String, RawModule>,
}

impl RawModule {
    fn into_locked(self, alias: &str) -> Result<LockedModule, LockError> {
        let tree_sha256 = parse_digest(
            self.tree_sha256,
            || LockError::ModuleTreeHash {
                alias: alias.to_owned(),
            },
            || LockError::ModuleTreeHashFormat {
                alias: alias.to_owned(),
            },
        )?;
        let modules = convert_map(self.modules, |child_alias, child| {
            let scoped = format!("{alias}/{child_alias}");
            child.into_locked(&scoped)
        })?;
        match (self.git, self.path, self.tarball) {
            (Some(git), None, None) => {
                if self.tarball_sha256.is_some() {
                    return Err(LockError::ModuleSourceFields {
                        alias: alias.to_owned(),
                    });
                }
                if git.is_empty() {
                    return Err(LockError::ModuleRepository {
                        alias: alias.to_owned(),
                    });
                }
                let revision = self
                    .revision
                    .map(|rev| normalize_revision(rev, alias))
                    .transpose()?;
                let commit = self
                    .commit
                    .ok_or_else(|| LockError::ModuleCommit {
                        alias: alias.to_owned(),
                    })?
                    .to_ascii_lowercase();
                if !is_commit(&commit) {
                    return Err(LockError::ModuleCommitFormat {
                        alias: alias.to_owned(),
                    });
                }
                let subdir = self.subdir.unwrap_or_default();
                if !is_contained(&subdir) {
                    return Err(LockError::ModuleSubdir {
                        alias: alias.to_owned(),
                    });
                }
                Ok(LockedModule::Git {
                    git,
                    revision,
                    subdir,
                    commit,
                    tree_sha256,
                    modules,
                })
            }
            (None, Some(path), None) => {
                if self.tarball_sha256.is_some() {
                    return Err(LockError::ModuleSourceFields {
                        alias: alias.to_owned(),
                    });
                }
                if self.revision.is_some() || self.commit.is_some() || self.subdir.is_some() {
                    return Err(LockError::LocalModuleFields {
                        alias: alias.to_owned(),
                    });
                }
                if path.as_os_str().is_empty() {
                    return Err(LockError::LocalModulePath {
                        alias: alias.to_owned(),
                    });
                }
                Ok(LockedModule::Local {
                    path,
                    tree_sha256,
                    modules,
                })
            }
            (None, None, Some(tarball)) => {
                if self.revision.is_some() || self.commit.is_some() || self.subdir.is_some() {
                    return Err(LockError::ModuleSourceFields {
                        alias: alias.to_owned(),
                    });
                }
                let empty = match &tarball {
                    TarballLocation::Url { url } => url.is_empty(),
                    TarballLocation::File { path } => path.as_os_str().is_empty(),
                };
                if empty {
                    return Err(LockError::ModuleTarballLocation {
                        alias: alias.to_owned(),
                    });
                }
                let tarball_sha256 = self
                    .tarball_sha256
                    .as_deref()
                    .and_then(Sha256Digest::from_hex)
                    .ok_or_else(|| LockError::ModuleTarballHash {
                        alias: alias.to_owned(),
                    })?;
                Ok(LockedModule::Tarball {
                    tarball,
                    tarball_sha256,
                    tree_sha256,
                    modules,
                })
            }
            (None, None, None) => Err(LockError::ModuleSourceMissing {
                alias: alias.to_owned(),
            }),
            _ => Err(LockError::ModuleSourceAmbiguous {
                alias: alias.to_owned(),
            }),
        }
    }
}

fn normalize_revision(rev: LockedRevision, alias: &str) -> Result<LockedRevision, LockError> {
    match rev {
        LockedRevision::Rev(val) => {
            let val = val.to_ascii_lowercase();
            if !is_commit(&val) {
                return Err(LockError::ModuleRevisionFormat {
                    alias: alias.to_owned(),
                });
            }
            Ok(LockedRevision::Rev(val))
        }
        other => Ok(other),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackage {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    tree_sha256: Option<String>,
}

impl RawPackage {
    fn into_locked(self, name: &str) -> Result<LockedPackage, LockError> {
        let version = require_nonempty(self.version, || LockError::PackageVersion {
            name: name.to_owned(),
        })?;
        let tree_sha256 = parse_digest(
            self.tree_sha256,
            || LockError::PackageTreeHash {
                name: name.to_owned(),
            },
            || LockError::PackageTreeHashFormat {
                name: name.to_owned(),
            },
        )?;
        Ok(LockedPackage {
            version,
            tree_sha256,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIcon {
    #[serde(default)]
    source: Option<IconSource>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

impl RawIcon {
    fn into_locked(self, name: &str) -> Result<LockedIcon, LockError> {
        let source = self.source.ok_or_else(|| LockError::IconSource {
            name: name.to_owned(),
        })?;
        let url = require_nonempty(self.url, || LockError::IconUrl {
            name: name.to_owned(),
        })?;
        let sha256 = parse_digest(
            self.sha256,
            || LockError::IconDigest {
                name: name.to_owned(),
            },
            || LockError::IconDigestFormat {
                name: name.to_owned(),
            },
        )?;
        let size = self.size.ok_or_else(|| LockError::IconSize {
            name: name.to_owned(),
        })?;
        Ok(LockedIcon {
            source,
            url,
            sha256,
            size,
        })
    }
}

fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

fn is_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_contained(path: &Path) -> bool {
    path.components()
        .all(|c| matches!(c, Component::CurDir | Component::Normal(_)))
}

fn legacy_error(text: &str) -> Option<LockError> {
    let val: toml::Value = toml::from_str(text).ok()?;
    for key in ["version", "lock-version", "schema-version"] {
        if val.get(key).is_some() {
            return Some(LockError::FormatVersion);
        }
    }
    let modules = val.get("modules")?.as_table()?;
    if modules.values().any(|m| {
        m.as_table()
            .is_some_and(|t| t.contains_key("repository") || !t.contains_key("tree_sha256"))
    }) {
        return Some(LockError::Legacy);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_module_lock_roundtrip_and_empty_omitted() {
        let mut lock = InputLock::default();
        let digest = Sha256Digest::of_bytes(b"mod");
        let child_digest = Sha256Digest::of_bytes(b"child");
        let mut children = BTreeMap::new();
        children.insert(
            "helper".to_owned(),
            LockedModule::Git {
                git: "https://example.org/helper.git".into(),
                revision: Some(LockedRevision::Tag("v1".into())),
                subdir: PathBuf::new(),
                commit: "0123456789abcdef0123456789abcdef01234567".into(),
                tree_sha256: child_digest,
                modules: BTreeMap::new(),
            },
        );
        lock.modules.insert(
            "docs".to_owned(),
            LockedModule::Local {
                path: PathBuf::from("theme"),
                tree_sha256: digest,
                modules: children,
            },
        );
        lock.modules.insert(
            "plain".to_owned(),
            LockedModule::Local {
                path: PathBuf::from("plain"),
                tree_sha256: digest,
                modules: BTreeMap::new(),
            },
        );

        let encoded = lock.to_toml().unwrap();
        assert!(encoded.contains("[modules.docs.modules.helper]"));
        assert!(!encoded.contains("[modules.plain.modules]"));

        let decoded = InputLock::parse(&encoded).unwrap();
        assert_eq!(decoded, lock);
    }
}
