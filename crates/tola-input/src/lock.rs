//! `tola.lock`: the resolved commit, version, and digest of every input a site selects.
//!
//! The lock records what a module resolved to, which version of a package the compiler read,
//! and the digest of the exact bytes an icon collection arrived as.
//! Strict parsing means a lock from an older format fails with a named repair instead of being
//! read as if it still described this site.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::hash::Sha256Digest;

/// The file name of the lock, relative to the site root.
pub const LOCK_FILE: &str = "tola.lock";

/// Every input a site resolved, keyed by the name the site refers to it by.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct InputLock {
    /// Remote and local modules, keyed by alias.
    pub modules: BTreeMap<String, LockedModule>,
    /// Typst packages the compiler read, keyed by `@namespace/name`.
    pub packages: BTreeMap<String, LockedPackage>,
    /// Remote icon collections, keyed by namespace.
    pub icons: BTreeMap<String, LockedIcon>,
}

impl InputLock {
    /// The lock's path under one site root.
    pub fn path(site_root: &Path) -> PathBuf {
        site_root.join(LOCK_FILE)
    }

    /// Read the lock of one site, or `None` when the site has never resolved its inputs.
    pub fn read(site_root: &Path) -> Result<Option<Self>, LockError> {
        let path = Self::path(site_root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(LockError::Read { path, source }),
        };
        Self::parse(&text).map(Some)
    }

    /// Parse one lock, naming a legacy layout instead of reporting an unknown field.
    pub fn parse(text: &str) -> Result<Self, LockError> {
        let raw: RawLock = match toml::from_str(text) {
            Ok(raw) => raw,
            Err(source) => {
                if let Some(error) = legacy_error(text) {
                    return Err(error);
                }
                return Err(LockError::Parse { source });
            }
        };
        raw.into_lock()
    }

    /// The lock's TOML text.
    pub fn to_toml(&self) -> Result<String, LockError> {
        toml::to_string_pretty(self).map_err(|source| LockError::Encode { source })
    }

    /// Replace the lock in `site_root`, leaving the previous file in place on any failure.
    pub fn write_atomic(&self, site_root: &Path) -> Result<(), LockError> {
        let text = self.to_toml()?;
        let path = Self::path(site_root);
        let mut temporary = tempfile::NamedTempFile::new_in(site_root)
            .map_err(|source| LockError::Stage { source })?;
        temporary
            .write_all(text.as_bytes())
            .map_err(|source| LockError::Write { source })?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|source| LockError::Flush { source })?;
        temporary
            .persist(&path)
            .map_err(|error| LockError::Replace {
                path,
                source: error.error,
            })?;
        Ok(())
    }
}

/// The failure of reading, parsing, or writing the lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("Tola could not read `{path}`")]
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
    #[error("Tola could not replace `{path}`")]
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

/// One module's resolved identity.
///
/// A remote module records the commit its selector resolved to; a local module is site source and
/// its digest only proves what a build read. Both serialize flat, as the lock format requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum LockedModule {
    /// A module fetched from a Git repository.
    Git {
        /// The repository the module resolved from.
        git: String,
        /// The selector the module was resolved through; `None` selected the default branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<LockedRevision>,
        /// The module's directory inside the repository; empty for the repository root.
        #[serde(default, skip_serializing_if = "path_is_empty")]
        subdir: PathBuf,
        /// The commit the selector resolved to.
        commit: String,
        /// The digest of the module directory this build reads.
        tree_sha256: Sha256Digest,
    },
    /// A module declared by a path inside the site.
    Local {
        /// The module's directory, relative to the site root.
        path: PathBuf,
        /// The digest of the module directory this build reads.
        tree_sha256: Sha256Digest,
    },
    /// A downloaded or local module archive and its expanded contents.
    Tarball {
        tarball: TarballLocation,
        tarball_sha256: Sha256Digest,
        tree_sha256: Sha256Digest,
    },
}

/// Where the compressed bytes of a module are obtained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum TarballLocation {
    Url { url: String },
    File { path: PathBuf },
}

impl TarballLocation {
    /// Reject credentials and ambiguous locations before they can enter a lock or a request.
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
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(
            deny_unknown_fields,
            expecting = "a table with exactly one of `url` or `path`"
        )]
        struct Fields {
            url: Option<String>,
            path: Option<PathBuf>,
        }
        let fields = Fields::deserialize(deserializer)?;
        let location = match (fields.url, fields.path) {
            (Some(url), None) => Self::Url { url },
            (None, Some(path)) => Self::File { path },
            _ => {
                return Err(serde::de::Error::custom(
                    "choose exactly one of `tarball.url` or `tarball.path`",
                ));
            }
        };
        location.validate().map_err(serde::de::Error::custom)?;
        Ok(location)
    }
}

impl LockedModule {
    /// The digest of the module directory this build reads.
    pub fn tree_sha256(&self) -> Sha256Digest {
        match self {
            Self::Git { tree_sha256, .. }
            | Self::Local { tree_sha256, .. }
            | Self::Tarball { tree_sha256, .. } => *tree_sha256,
        }
    }
}

/// The revision selector a remote module declaration asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockedRevision {
    Tag(String),
    Branch(String),
    Rev(String),
}

/// One Typst package version the compiler read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    /// The package version the compiler selected.
    pub version: String,
    /// The digest of the package directory.
    pub tree_sha256: Sha256Digest,
}

/// One remote icon collection's resolved identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedIcon {
    /// The declaration the collection resolved from.
    pub source: IconSource,
    /// The URL the collection's bytes came from.
    pub url: String,
    /// The digest of those exact bytes.
    pub sha256: Sha256Digest,
    /// The length of those exact bytes.
    pub size: u64,
}

/// The declaration one icon collection resolved from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconSource {
    /// A collection named by a preset and the release it resolved to.
    Preset { preset: String, version: String },
    /// A collection named by a bare URL.
    Url { url: String },
}

/// The lock as written, before any field is trusted.
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
        let mut modules = BTreeMap::new();
        for (alias, module) in self.modules {
            modules.insert(alias.clone(), module.into_locked(&alias)?);
        }
        let mut packages = BTreeMap::new();
        for (name, package) in self.packages {
            packages.insert(name.clone(), package.into_locked(&name)?);
        }
        let mut icons = BTreeMap::new();
        for (name, icon) in self.icons {
            icons.insert(name.clone(), icon.into_locked(&name)?);
        }
        Ok(InputLock {
            modules,
            packages,
            icons,
        })
    }
}

/// One module entry as written.
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
}

impl RawModule {
    fn into_locked(self, alias: &str) -> Result<LockedModule, LockError> {
        let tree_sha256 = match self.tree_sha256 {
            Some(digest) => {
                Sha256Digest::from_hex(&digest).ok_or_else(|| LockError::ModuleTreeHashFormat {
                    alias: alias.to_owned(),
                })?
            }
            None => {
                return Err(LockError::ModuleTreeHash {
                    alias: alias.to_owned(),
                });
            }
        };
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
                    .map(|revision| normalize_revision(revision, alias))
                    .transpose()?;
                let commit = self.commit.ok_or_else(|| LockError::ModuleCommit {
                    alias: alias.to_owned(),
                })?;
                let commit = commit.to_ascii_lowercase();
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
                Ok(LockedModule::Local { path, tree_sha256 })
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

/// The lock's revision selector with its commit form normalized.
fn normalize_revision(revision: LockedRevision, alias: &str) -> Result<LockedRevision, LockError> {
    match revision {
        LockedRevision::Rev(value) => {
            let value = value.to_ascii_lowercase();
            if !is_commit(&value) {
                return Err(LockError::ModuleRevisionFormat {
                    alias: alias.to_owned(),
                });
            }
            Ok(LockedRevision::Rev(value))
        }
        revision => Ok(revision),
    }
}

/// One package entry as written.
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
        let version = match self.version {
            Some(version) if !version.is_empty() => version,
            _ => {
                return Err(LockError::PackageVersion {
                    name: name.to_owned(),
                });
            }
        };
        let tree_sha256 = match self.tree_sha256 {
            Some(digest) => {
                Sha256Digest::from_hex(&digest).ok_or_else(|| LockError::PackageTreeHashFormat {
                    name: name.to_owned(),
                })?
            }
            None => {
                return Err(LockError::PackageTreeHash {
                    name: name.to_owned(),
                });
            }
        };
        Ok(LockedPackage {
            version,
            tree_sha256,
        })
    }
}

/// One icon collection entry as written.
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
        let url = match self.url {
            Some(url) if !url.is_empty() => url,
            _ => {
                return Err(LockError::IconUrl {
                    name: name.to_owned(),
                });
            }
        };
        let sha256 = match self.sha256 {
            Some(digest) => {
                Sha256Digest::from_hex(&digest).ok_or_else(|| LockError::IconDigestFormat {
                    name: name.to_owned(),
                })?
            }
            None => {
                return Err(LockError::IconDigest {
                    name: name.to_owned(),
                });
            }
        };
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

/// Whether one module directory inside its repository is unset.
fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// Whether `commit` is a full commit ID.
fn is_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `path` is relative and climbs no parent directory.
fn is_contained(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::CurDir | Component::Normal(_)))
}

/// A repair for a lock written before the input model recorded integrity.
fn legacy_error(text: &str) -> Option<LockError> {
    let value: toml::Value = toml::from_str(text).ok()?;
    for key in ["version", "lock-version", "schema-version"] {
        if value.get(key).is_some() {
            return Some(LockError::FormatVersion);
        }
    }
    let modules = value.get("modules")?.as_table()?;
    if modules.values().any(|module| {
        module.as_table().is_some_and(|module| {
            module.contains_key("repository") || !module.contains_key("tree_sha256")
        })
    }) {
        return Some(LockError::Legacy);
    }
    None
}
