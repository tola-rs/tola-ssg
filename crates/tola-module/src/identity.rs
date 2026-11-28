//! Strongly-typed module identities, capabilities, and Tola minimum version constraints.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::assets::Assets;
use crate::declaration::{valid_name, valid_scoped_name};
use crate::hooks::ModuleHooks;
use crate::icons::Icons;
use crate::manifest::TypstInputs;

/// Error returned when a module alias or scoped module path violates naming rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a usable module name")]
pub struct InvalidModuleName(pub String);

/// A validated single-segment module alias (no `/`), as declared under `[modules.<alias>]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ModuleAlias(String);

impl ModuleAlias {
    /// Parse a single lowercase kebab-case module alias.
    pub fn parse(raw: &str) -> Result<Self, InvalidModuleName> {
        if valid_name(raw) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InvalidModuleName(raw.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ModuleAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ModuleAlias {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ModuleAlias {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for ModuleAlias {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ModuleAlias {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl FromStr for ModuleAlias {
    type Err = InvalidModuleName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for ModuleAlias {
    type Error = InvalidModuleName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if valid_name(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidModuleName(value))
        }
    }
}

/// Validated scoped module identity (`docs` or `docs/tailwind`), distinguishing
/// site-declared modules from private child modules without ad-hoc `/` splitting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ModuleId(String);

impl ModuleId {
    /// Wrap a validated top-level alias as a [`ModuleId`].
    pub fn top_level(alias: ModuleAlias) -> Self {
        Self(alias.into_string())
    }

    /// Parse a top-level or slash-separated scoped module identity.
    pub fn parse(raw: &str) -> Result<Self, InvalidModuleName> {
        if valid_scoped_name(raw) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InvalidModuleName(raw.to_owned()))
        }
    }

    /// Construct the child identity `{self}/{sub}`.
    pub fn child(&self, sub: &ModuleAlias) -> Self {
        Self(format!("{}/{sub}", self.0))
    }

    /// Whether this identity names a module declared directly by the site.
    pub fn is_top_level(&self) -> bool {
        !self.0.contains('/')
    }

    /// The trailing local alias declared in its owner's `[modules.<alias>]`.
    pub fn leaf(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The owning parent identity, or `None` when top-level.
    pub fn parent(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(parent, _)| parent)
    }

    /// Alias segments from root owner to leaf.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ModuleId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ModuleId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for ModuleId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ModuleId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl From<ModuleAlias> for ModuleId {
    fn from(alias: ModuleAlias) -> Self {
        Self::top_level(alias)
    }
}

impl FromStr for ModuleId {
    type Err = InvalidModuleName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for ModuleId {
    type Error = InvalidModuleName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if valid_scoped_name(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidModuleName(value))
        }
    }
}

/// Capabilities automatically derived from the sections declared in `tola-module.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleCapability {
    SiteEntry,
    TypstLibrary,
    TypstFonts,
    NuLibrary,
    Assets,
    Icons,
    Hooks,
    Commands,
    Queries,
    Initializer,
    Modules,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn derive_capabilities(
    has_build: bool,
    typst: Option<&TypstInputs>,
    has_nu: bool,
    assets: Option<&Assets>,
    icons: &Icons,
    hooks: &ModuleHooks,
    has_commands: bool,
    has_queries: bool,
    has_init: bool,
    has_modules: bool,
) -> Vec<ModuleCapability> {
    let mut caps = Vec::new();
    if has_build {
        caps.push(ModuleCapability::SiteEntry);
    }
    if let Some(typst) = typst {
        if typst.entry.is_some() {
            caps.push(ModuleCapability::TypstLibrary);
        }
        if !typst.fonts.is_empty() {
            caps.push(ModuleCapability::TypstFonts);
        }
    }
    if has_nu {
        caps.push(ModuleCapability::NuLibrary);
    }
    if assets.is_some_and(|a| !a.files.is_empty() || !a.trees.is_empty()) {
        caps.push(ModuleCapability::Assets);
    }
    if !icons.collections.is_empty() {
        caps.push(ModuleCapability::Icons);
    }
    if !hooks.is_empty() {
        caps.push(ModuleCapability::Hooks);
    }
    if has_commands {
        caps.push(ModuleCapability::Commands);
    }
    if has_queries {
        caps.push(ModuleCapability::Queries);
    }
    if has_init {
        caps.push(ModuleCapability::Initializer);
    }
    if has_modules {
        caps.push(ModuleCapability::Modules);
    }
    caps
}

/// Validated `MAJOR.MINOR.PATCH[-pre]` Tola version used by `min-tola-version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TolaVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl TolaVersion {
    /// The version of the running Tola binary/crate.
    pub fn current() -> Self {
        env!("CARGO_PKG_VERSION")
            .parse()
            .expect("CARGO_PKG_VERSION is valid semver")
    }
}

impl FromStr for TolaVersion {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (core, pre) = match raw.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
            Some(_) => {
                return Err(format!(
                    "`min-tola-version` must be a `MAJOR.MINOR.PATCH` version string (got `{raw}`)"
                ));
            }
            None => (raw, None),
        };
        let mut parts = core.split('.');
        let invalid = || {
            format!("`min-tola-version` must be a `MAJOR.MINOR.PATCH` version string (got `{raw}`)")
        };
        let major = parts
            .next()
            .ok_or_else(invalid)?
            .parse::<u64>()
            .map_err(|_| invalid())?;
        let minor = parts
            .next()
            .ok_or_else(invalid)?
            .parse::<u64>()
            .map_err(|_| invalid())?;
        let patch = parts
            .next()
            .ok_or_else(invalid)?
            .parse::<u64>()
            .map_err(|_| invalid())?;
        if parts.next().is_some() {
            return Err(invalid());
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl Ord for TolaVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for TolaVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for TolaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl Serialize for TolaVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TolaVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_and_id_enforce_scope_rules() {
        let docs = ModuleAlias::parse("docs").unwrap();
        let tw = ModuleAlias::parse("tailwind").unwrap();
        assert!(ModuleAlias::parse("docs/tailwind").is_err());
        assert!(ModuleAlias::parse("").is_err());
        assert!(ModuleAlias::parse("Docs").is_err());

        let top = ModuleId::top_level(docs);
        assert!(top.is_top_level());
        assert_eq!(top.leaf(), "docs");
        assert_eq!(top.parent(), None);

        let child = top.child(&tw);
        assert!(!child.is_top_level());
        assert_eq!(child.as_str(), "docs/tailwind");
        assert_eq!(child.leaf(), "tailwind");
        assert_eq!(child.parent(), Some("docs"));
        assert_eq!(child.segments().collect::<Vec<_>>(), ["docs", "tailwind"]);

        assert!(ModuleId::parse("docs//tailwind").is_err());
    }

    #[test]
    fn tola_version_parses_and_orders() {
        let rc: TolaVersion = "0.9.0-rc.1".parse().unwrap();
        let rel: TolaVersion = "0.9.0".parse().unwrap();
        let next: TolaVersion = "0.10.0".parse().unwrap();
        assert!(rc < rel);
        assert!(rel < next);
        assert_eq!(rc.to_string(), "0.9.0-rc.1");
        assert!("0.9".parse::<TolaVersion>().is_err());
        assert!("0.9.0-".parse::<TolaVersion>().is_err());
    }
}
