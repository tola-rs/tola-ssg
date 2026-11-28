//! Module declaration types and their validation.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::action::Action;
use crate::fragment::{FragmentError, FragmentStep, optional_table, read_fields, read_fragment};

/// A module selected by the site.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModuleDeclaration {
    #[serde(flatten)]
    pub source: ModuleSource,
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub options: toml::Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<ModuleAssetRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<crate::icons::Icons>,
}

/// A private child module declared inside another module's manifest.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChildModuleDeclaration {
    #[serde(flatten)]
    pub source: ModuleSource,
    #[serde(
        default,
        rename = "enabled-by",
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled_by: Option<crate::options::OptionPath>,
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub options: toml::Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<ModuleAssetRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<crate::icons::Icons>,
}

impl ChildModuleDeclaration {
    /// Project this child declaration into a plain module declaration without its parent toggle.
    pub fn into_module_declaration(self) -> ModuleDeclaration {
        ModuleDeclaration {
            source: self.source,
            options: self.options,
            assets: self.assets,
            icons: self.icons,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ModuleSource {
    Local { path: PathBuf },
    Git { git: GitSource },
    Tarball { tarball: TarballSource },
}

/// One prepared module obtained from a tar.gz, tar.zst, or ZIP file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TarballSource {
    #[serde(flatten)]
    pub location: tola_input::TarballLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<tola_input::Sha256Digest>,
}

impl<'de> Deserialize<'de> for TarballSource {
    fn deserialize<D: serde::Deserializer<'de>>(deser: D) -> Result<Self, D::Error> {
        Self::try_from(read_fields::<TarballFields, _>(deser)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(expecting = "a tarball table with `url` or `path`, and optional `sha256`")]
struct TarballFields {
    url: Option<String>,
    path: Option<PathBuf>,
    sha256: Option<tola_input::Sha256Digest>,
}

impl TryFrom<TarballFields> for TarballSource {
    type Error = &'static str;

    fn try_from(fields: TarballFields) -> Result<Self, Self::Error> {
        let loc = match (fields.url, fields.path) {
            (Some(url), None) => tola_input::TarballLocation::Url { url },
            (None, Some(path)) if !path.as_os_str().is_empty() => {
                tola_input::TarballLocation::File { path }
            }
            _ => return Err("choose exactly one nonempty `tarball.url` or `tarball.path`"),
        };
        loc.validate()?;
        Ok(Self {
            location: loc,
            sha256: fields.sha256,
        })
    }
}

/// A repository, its revision selector, and the directory providing the module.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitSource {
    /// The HTTPS or SSH repository address.
    pub url: String,
    /// Absent selects the repository's default branch.
    #[serde(flatten)]
    pub revision: Option<RemoteRevision>,
    /// The module directory inside the repository; empty selects its root.
    #[serde(skip_serializing_if = "is_empty_path")]
    pub subdir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteRevision {
    Tag(String),
    Branch(String),
    Rev(String),
}

impl Default for ModuleDeclaration {
    fn default() -> Self {
        Self {
            source: ModuleSource::Local {
                path: PathBuf::new(),
            },
            options: toml::Table::new(),
            assets: None,
            icons: None,
        }
    }
}

impl<'de> Deserialize<'de> for ModuleDeclaration {
    fn deserialize<D: serde::Deserializer<'de>>(deser: D) -> Result<Self, D::Error> {
        Self::try_from(read_fields::<DeclFields, _>(deser)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DeclFields {
    path: Option<PathBuf>,
    #[serde(default, deserialize_with = "optional_table")]
    git: Option<GitFields>,
    #[serde(default, deserialize_with = "optional_table")]
    tarball: Option<TarballFields>,
    enabled_by: Option<crate::options::OptionPath>,
    options: Option<toml::Table>,
    #[serde(default, deserialize_with = "optional_table")]
    assets: Option<AssetRuleFields>,
    icons: Option<toml::Value>,
}

impl DeclFields {
    fn parse_parts(
        self,
        allow_enabled_by: bool,
    ) -> Result<
        (
            ModuleSource,
            Option<crate::options::OptionPath>,
            toml::Table,
            Option<ModuleAssetRules>,
            Option<crate::icons::Icons>,
        ),
        String,
    > {
        if !allow_enabled_by && self.enabled_by.is_some() {
            return Err(
                "`enabled-by` is only allowed on child modules inside `tola-module.toml`".into(),
            );
        }
        let mut opts = self.options.unwrap_or_default();
        crate::options::normalize_options(&mut opts).map_err(|err| err.to_string())?;
        let source = match (self.path, self.git, self.tarball) {
            (Some(path), None, None) if !path.as_os_str().is_empty() => {
                ModuleSource::Local { path }
            }
            (Some(_), None, None) => return Err("`path` must not be empty".into()),
            (None, Some(git), None) => ModuleSource::Git {
                git: git.try_into()?,
            },
            (None, None, Some(tarball)) => ModuleSource::Tarball {
                tarball: tarball.try_into()?,
            },
            _ => return Err("choose exactly one of `path`, `git`, or `tarball`".into()),
        };
        let assets = self.assets.map(Into::into);
        let icons = self
            .icons
            .map(toml::Value::try_into)
            .transpose()
            .map_err(|err: toml::de::Error| err.to_string())?;
        Ok((source, self.enabled_by, opts, assets, icons))
    }
}

impl TryFrom<DeclFields> for ModuleDeclaration {
    type Error = String;

    fn try_from(fields: DeclFields) -> Result<Self, Self::Error> {
        let (source, _, options, assets, icons) = fields.parse_parts(false)?;
        Ok(Self {
            source,
            options,
            assets,
            icons,
        })
    }
}

impl<'de> Deserialize<'de> for ChildModuleDeclaration {
    fn deserialize<D: serde::Deserializer<'de>>(deser: D) -> Result<Self, D::Error> {
        let fields = read_fields::<DeclFields, _>(deser)?;
        let (source, enabled_by, options, assets, icons) =
            fields.parse_parts(true).map_err(serde::de::Error::custom)?;
        Ok(Self {
            source,
            enabled_by,
            options,
            assets,
            icons,
        })
    }
}

impl ModuleDeclaration {
    /// Check authored fields before other configuration fragments fill in missing fields.
    pub fn validate_fragment(value: &toml::Value) -> Result<(), FragmentError> {
        let fields = read_fragment::<DeclFields>(value)?;
        if let Some(icons) = fields.icons {
            crate::icons::Icons::validate_fragment(&icons)
                .map_err(|err| err.at(FragmentStep::Key("icons".into())))?;
        }
        if let Some(mut opts) = fields.options {
            crate::options::normalize_options(&mut opts)
                .map_err(|err| err.at(FragmentStep::Key("options".into())))?;
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for GitSource {
    fn deserialize<D: serde::Deserializer<'de>>(deser: D) -> Result<Self, D::Error> {
        Self::try_from(read_fields::<GitFields, _>(deser)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
struct GitFields {
    url: Option<String>,
    tag: Option<String>,
    branch: Option<String>,
    rev: Option<String>,
    subdir: Option<PathBuf>,
}

impl TryFrom<GitFields> for GitSource {
    type Error = &'static str;

    fn try_from(fields: GitFields) -> Result<Self, Self::Error> {
        Ok(Self {
            url: validate_git_url(fields.url.ok_or("declare `git.url`")?)?,
            revision: select_rev(fields.tag, fields.branch, fields.rev)?,
            subdir: select_subdir(fields.subdir)?,
        })
    }
}

fn is_empty_path(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

fn validate_git_url(url: String) -> std::result::Result<String, &'static str> {
    const ADDRESS: &str = "`git.url` must be an `https` or `ssh` address";
    let parsed = gix::Url::try_from(url.as_str()).map_err(|_| ADDRESS)?;
    match parsed.scheme {
        gix::url::Scheme::Https | gix::url::Scheme::Ssh => Ok(url),
        _ => Err(ADDRESS),
    }
}

fn select_rev(
    tag: Option<String>,
    branch: Option<String>,
    rev: Option<String>,
) -> std::result::Result<Option<RemoteRevision>, &'static str> {
    match (tag, branch, rev) {
        (None, None, None) => Ok(None),
        (Some(tag), None, None) => Ok(Some(RemoteRevision::Tag(tag))),
        (None, Some(branch), None) => Ok(Some(RemoteRevision::Branch(branch))),
        (None, None, Some(rev)) => {
            let commit = gix::ObjectId::from_hex(rev.as_bytes())
                .map_err(|_| "`git.rev` must be a full commit ID")?;
            Ok(Some(RemoteRevision::Rev(commit.to_string())))
        }
        _ => Err("choose at most one of `git.tag`, `git.branch`, or `git.rev`"),
    }
}

fn select_subdir(subdir: Option<PathBuf>) -> std::result::Result<PathBuf, &'static str> {
    match subdir {
        Some(path) if is_relative_path(&path) => Ok(path),
        Some(_) => Err("`git.subdir` must be a relative path inside the repository"),
        None => Ok(PathBuf::new()),
    }
}

/// Project selection and replacement of one module's published assets.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModuleAssetRules {
    pub enable: bool,
    pub replace: BTreeMap<String, PathBuf>,
}

#[derive(Deserialize)]
struct AssetRuleFields {
    enable: Option<bool>,
    replace: Option<BTreeMap<String, PathBuf>>,
}

impl From<AssetRuleFields> for ModuleAssetRules {
    fn from(fields: AssetRuleFields) -> Self {
        Self {
            enable: fields.enable.unwrap_or(true),
            replace: fields.replace.unwrap_or_default(),
        }
    }
}

impl<'de> Deserialize<'de> for ModuleAssetRules {
    fn deserialize<D: serde::Deserializer<'de>>(deser: D) -> Result<Self, D::Error> {
        read_fields::<AssetRuleFields, _>(deser).map(Into::into)
    }
}

impl Default for ModuleAssetRules {
    fn default() -> Self {
        Self {
            enable: true,
            replace: BTreeMap::new(),
        }
    }
}

/// A module-relative Typst program.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub entry: PathBuf,
}

pub use crate::assets::Assets;

/// A private metadata query declared by a module.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Query {
    /// Source file relative to the module root.
    pub entry: PathBuf,
    pub label: String,
    #[serde(default)]
    pub references: Vec<FunctionParameter>,
    #[serde(default)]
    pub definitions: Vec<FunctionParameter>,
}

impl Query {
    /// Whether this query's records supply IDs for declared function parameters.
    pub fn has_entities(&self) -> bool {
        !self.references.is_empty() || !self.definitions.is_empty()
    }
}

/// One parameter of a function exported by the module's public Typst entry.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FunctionParameter {
    pub export: String,
    pub parameter: Parameter,
}

/// Select a positional parameter by its zero-based index or a named parameter by name.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged, deny_unknown_fields)]
pub enum Parameter {
    Positional { positional: usize },
    Named { named: String },
}

/// A command's statically inspectable description and action.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Command {
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    pub nu: Option<PathBuf>,
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub projection: Vec<String>,
    #[serde(default)]
    pub skill: Option<PathBuf>,
}

impl Command {
    /// Select and validate the command's declared action.
    pub fn action(&self) -> Result<CommandAction, CommandActionError> {
        if self.argv.is_none() && self.nu.is_none() && self.args.is_none() {
            match (&self.query, &self.skill) {
                (Some(query), None) => {
                    return Ok(CommandAction::Query {
                        query: query.clone(),
                        projection: self.projection.clone(),
                    });
                }
                (None, Some(path)) if self.projection.is_empty() => {
                    return Ok(CommandAction::Skill { path: path.clone() });
                }
                _ => {}
            }
        } else if self.query.is_none() && self.skill.is_none() && self.projection.is_empty() {
            return Action::from_fields(self.nu.clone(), self.argv.clone(), self.args.clone())
                .map(CommandAction::Execute)
                .map_err(CommandActionError);
        }
        Err(CommandActionError(
            "declare exactly one of `nu`, `argv`, `query`, or `skill`; `projection` only applies to `query`".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CommandActionError(String);

/// A module-owned initializer invoked before a site exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initializer {
    pub action: Action,
}

impl<'de> Deserialize<'de> for Initializer {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            argv: Option<Vec<String>>,
            nu: Option<PathBuf>,
            args: Option<Vec<String>>,
        }

        let fields = Fields::deserialize(deserializer)?;
        let action = Action::from_fields(fields.nu, fields.argv, fields.args)
            .map_err(serde::de::Error::custom)?;
        Ok(Self { action })
    }
}

/// A program, declared query, or help file invoked as a module command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Execute(Action),
    Query {
        query: String,
        projection: Vec<String>,
    },
    Skill {
        path: PathBuf,
    },
}

pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `name` is one alias or slash-separated scoped aliases (`parent/child`).
pub fn valid_scoped_name(name: &str) -> bool {
    !name.is_empty() && name.split('/').all(valid_name)
}

/// Whether `path` is a nonempty spelling of plain path segments: no `..`, root, or prefix.
pub fn is_relative_path(path: &Path) -> bool {
    tola_input::validate_relative_path(path).is_ok()
}

pub(crate) fn is_nonempty_relative_path(path: &Path) -> bool {
    is_relative_path(path)
        && path
            .components()
            .any(|part| matches!(part, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::FragmentErrorKind;

    #[test]
    fn git_decl_roundtrip() {
        let decl: ModuleDeclaration = toml::from_str(
            r#"git = { url = "https://example.com/theme.git", subdir = "theme", tag = "v1" }
options = { enabled = true }"#,
        )
        .unwrap();
        let ModuleSource::Git { git } = &decl.source else {
            panic!("expected git source")
        };
        assert_eq!(git.subdir, Path::new("theme"));
        assert_eq!(git.revision, Some(RemoteRevision::Tag("v1".into())));
        let encoded = toml::to_string(&decl).unwrap();
        assert_eq!(toml::from_str::<ModuleDeclaration>(&encoded).unwrap(), decl);
    }

    #[test]
    fn git_decl_rejects_invalid_sources() {
        for src in [
            r#"git = "https://example.com/theme.git""#,
            "git = { url = 'https://example.com/theme.git' }\ntag = 'v1'",
            "path = 'theme'\ngit = { url = 'https://example.com/theme.git' }",
            "git = { url = 'https://example.com/theme.git', tag = 'v1', branch = 'main' }",
            "git = { url = 'https://example.com/theme.git', subdir = '../outside' }",
            "git = { url = 'https://example.com/theme.git', rev = 'short' }",
        ] {
            assert!(toml::from_str::<ModuleDeclaration>(src).is_err(), "{src}");
        }
    }

    #[test]
    fn fragment_defers_required_and_rejects_unknown() {
        for src in [
            "git = { tag = 'v1' }",
            "tarball = { sha256 = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' }",
            "[options]\n'posted.on' = 2026-11-15",
        ] {
            let val: toml::Value = toml::from_str(src).unwrap();
            ModuleDeclaration::validate_fragment(&val).unwrap();
            assert!(toml::from_str::<ModuleDeclaration>(src).is_err());
        }

        let src = "git = { url = 'https://example.org/theme', 'wrong.key' = true }";
        let val = toml::from_str(src).unwrap();
        let err = ModuleDeclaration::validate_fragment(&val).unwrap_err();
        assert!(matches!(err.kind, FragmentErrorKind::UnknownKey));
        assert_eq!(
            err.path,
            [
                FragmentStep::Key("git".into()),
                FragmentStep::Key("wrong.key".into()),
            ]
        );
        assert!(toml::from_str::<ModuleDeclaration>(src).is_err());
    }

    #[test]
    fn fragment_rejects_wrong_types() {
        for (src, path) in [
            ("git = { tag = 2026-11-15 }", vec!["git", "tag"]),
            ("git = ['https://example.org/theme']", vec!["git"]),
        ] {
            let val = toml::from_str(src).unwrap();
            let err = ModuleDeclaration::validate_fragment(&val).unwrap_err();
            assert!(matches!(err.kind, FragmentErrorKind::InvalidValue(_)));
            assert_eq!(
                err.path,
                path.into_iter()
                    .map(|k| FragmentStep::Key(k.into()))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn command_action_selects_one() {
        let argv: Command = toml::from_str(r#"argv = ["editor"]"#).unwrap();
        assert_eq!(
            argv.action().unwrap(),
            CommandAction::Execute(Action::Argv(vec!["editor".into()]))
        );

        let nu: Command = toml::from_str("nu = \"tools/edit.nu\"\nargs = [\"--wait\"]").unwrap();
        assert_eq!(
            nu.action().unwrap(),
            CommandAction::Execute(Action::Nu {
                path: PathBuf::from("tools/edit.nu"),
                args: vec!["--wait".into()],
            }),
        );

        let query: Command = toml::from_str("query = \"nodes\"\nprojection = [\"id\"]").unwrap();
        assert_eq!(
            query.action().unwrap(),
            CommandAction::Query {
                query: "nodes".into(),
                projection: vec!["id".into()]
            }
        );

        let skill: Command = toml::from_str(r#"skill = "guide.md""#).unwrap();
        assert_eq!(
            skill.action().unwrap(),
            CommandAction::Skill {
                path: PathBuf::from("guide.md")
            }
        );
    }

    #[test]
    fn command_action_rejects_invalid() {
        for src in [
            "argv = [\"editor\"]\nquery = \"nodes\"",
            "argv = [\"editor\"]\nprojection = [\"id\"]",
        ] {
            let cmd: Command = toml::from_str(src).unwrap();
            assert!(cmd.action().is_err(), "{src}");
        }
        assert!(toml::from_str::<Command>("kind = \"argv\"\nargv = [\"editor\"]").is_err());
    }
}
