//! Module declaration types and their validation.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A module selected by the site.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModuleDeclaration {
    #[serde(flatten)]
    pub source: ModuleSource,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, toml::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<ModuleAssetPolicy>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ModuleSource {
    Local {
        path: PathBuf,
    },
    Remote {
        git: String,
        /// The declared selector; absent selects the repository's default branch.
        #[serde(flatten)]
        revision: Option<RemoteRevision>,
        /// The module's directory inside the repository; empty selects the repository root.
        #[serde(skip_serializing_if = "path_is_empty")]
        subdir: PathBuf,
    },
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
            options: BTreeMap::new(),
            overrides: BTreeMap::new(),
            assets: None,
        }
    }
}

impl<'de> Deserialize<'de> for ModuleDeclaration {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            path: Option<PathBuf>,
            git: Option<String>,
            /// A key this model replaced; accepted only to name the repair.
            repository: Option<String>,
            tag: Option<String>,
            branch: Option<String>,
            rev: Option<String>,
            subdir: Option<PathBuf>,
            #[serde(default)]
            options: BTreeMap<String, toml::Value>,
            #[serde(default)]
            overrides: BTreeMap<String, PathBuf>,
            assets: Option<ModuleAssetPolicy>,
        }
        let fields = Fields::deserialize(deserializer)?;
        if fields.repository.is_some() {
            return Err(serde::de::Error::custom(
                "`repository` was renamed to `git`; rename the key to `git`",
            ));
        }
        let source = match (fields.path, fields.git) {
            (Some(path), None) => {
                if fields.tag.is_some()
                    || fields.branch.is_some()
                    || fields.rev.is_some()
                    || fields.subdir.is_some()
                {
                    return Err(serde::de::Error::custom(
                        "only a `git` module may declare `tag`, `branch`, `rev`, or `subdir`",
                    ));
                }
                if path.as_os_str().is_empty() {
                    return Err(serde::de::Error::custom("`path` must not be empty"));
                }
                ModuleSource::Local { path }
            }
            (None, Some(git)) => ModuleSource::Remote {
                git: validate_git(git).map_err(serde::de::Error::custom)?,
                revision: select_revision(fields.tag, fields.branch, fields.rev)
                    .map_err(serde::de::Error::custom)?,
                subdir: select_subdir(fields.subdir).map_err(serde::de::Error::custom)?,
            },
            _ => {
                return Err(serde::de::Error::custom(
                    "choose exactly one of `path` or `git`",
                ));
            }
        };
        Ok(Self {
            source,
            options: fields.options,
            overrides: fields.overrides,
            assets: fields.assets,
        })
    }
}

/// Whether one module directory inside its repository is unset.
fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// Validate a repository address: only an `https` or `ssh` address names a fetchable module.
fn validate_git(git: String) -> std::result::Result<String, &'static str> {
    const ADDRESS: &str = "a module `git` must be an `https` or `ssh` address";
    let parsed = gix::Url::try_from(git.as_str()).map_err(|_| ADDRESS)?;
    match parsed.scheme {
        gix::url::Scheme::Https | gix::url::Scheme::Ssh => Ok(git),
        _ => Err(ADDRESS),
    }
}

/// Pick the one revision selector a `git` module declared, if it declared any.
fn select_revision(
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
                .map_err(|_| "`rev` must be a full commit ID")?;
            Ok(Some(RemoteRevision::Rev(commit.to_string())))
        }
        _ => Err("choose at most one of `tag`, `branch`, or `rev`"),
    }
}

/// Validate the module's directory inside its repository.
fn select_subdir(subdir: Option<PathBuf>) -> std::result::Result<PathBuf, &'static str> {
    match subdir {
        Some(path) if is_relative_path(&path) => Ok(path),
        Some(_) => Err("`subdir` must be a relative path inside the repository"),
        None => Ok(PathBuf::new()),
    }
}

/// Project selection and replacement of one module's published assets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModuleAssetPolicy {
    pub enabled: bool,
    pub include: Option<Vec<String>>,
    pub exclude: Vec<String>,
    pub replace: BTreeMap<String, PathBuf>,
}

impl Default for ModuleAssetPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            include: None,
            exclude: Vec::new(),
            replace: BTreeMap::new(),
        }
    }
}

/// Selects the module providing the site's entry point.
#[derive(Debug, Clone, Default, Serialize, Deserialize, tola_config::Config)]
#[serde(default, deny_unknown_fields)]
#[config(section = "theme")]
pub struct ThemeConfig {
    /// Namespace of the module providing this site.
    pub module: Option<String>,
}

impl ThemeConfig {
    pub const HELP: &str = "Select one declared module whose `[site].entry` generates the site.\n\n\
```toml\n[theme]\nmodule = \"notes\"\n\n[modules.notes]\npath = \"theme\"\n```\n\n\
The project needs no copied entry program. An explicitly configured `[build].entry` conflicts\n\
with this selection. Other modules can still provide libraries, queries, and commands.\n\
Read `[modules]` for module manifests, options, paths, and public imports.";
}

/// A module-relative Typst program.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub entry: PathBuf,
}

/// A module-relative Typst schema and the value it exports.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OptionsSchema {
    pub schema: PathBuf,
    pub export: String,
}

/// A module-relative asset directory.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assets {
    pub directory: PathBuf,
}

/// A module-authored authoring guide, printed by a `skill` command.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub path: PathBuf,
}

/// A private metadata query declared by a module.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Query {
    /// Source file relative to the module root.
    pub entry: PathBuf,
    pub label: String,
    /// The shape of the metadata value returned by this query.
    #[serde(default)]
    pub format: QueryFormat,
}

/// The supported query result contracts.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QueryFormat {
    #[default]
    Json,
    Entities,
}

/// A command's statically inspectable description and action.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Command {
    #[serde(default)]
    pub help: String,
    #[serde(flatten)]
    pub action: CommandAction,
}

/// A module-owned site initializer, invoked through the same argv convention as commands.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Initializer {
    pub argv: Vec<String>,
}

/// External commands execute an explicit argv; query commands name a declared query.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandAction {
    Argv {
        argv: Vec<String>,
    },
    Query {
        query: String,
        #[serde(default)]
        projection: Vec<String>,
    },
    Skill,
}

pub(crate) fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `path` is a nonempty spelling of plain path segments: no `..`, root, or prefix.
pub(crate) fn is_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}
