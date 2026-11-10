//! Module declaration types and their validation.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::action::Action;

/// A module selected by the site.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModuleDeclaration {
    #[serde(flatten)]
    pub source: ModuleSource,
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub options: toml::Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<ModuleAssetPolicy>,
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
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(
            deny_unknown_fields,
            expecting = "a tarball table with `url` or `path`, and optional `sha256`"
        )]
        struct Fields {
            url: Option<String>,
            path: Option<PathBuf>,
            sha256: Option<tola_input::Sha256Digest>,
        }
        let fields = Fields::deserialize(deserializer)?;
        let location = match (fields.url, fields.path) {
            (Some(url), None) => tola_input::TarballLocation::Url { url },
            (None, Some(path)) if !path.as_os_str().is_empty() => {
                tola_input::TarballLocation::File { path }
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "choose exactly one nonempty `tarball.url` or `tarball.path`",
                ));
            }
        };
        location.validate().map_err(serde::de::Error::custom)?;
        Ok(Self {
            location,
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
    #[serde(skip_serializing_if = "path_is_empty")]
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
            git: Option<GitSource>,
            tarball: Option<TarballSource>,
            #[serde(default, deserialize_with = "crate::options::deserialize_options")]
            options: toml::Table,
            assets: Option<ModuleAssetPolicy>,
        }
        let fields = Fields::deserialize(deserializer)?;
        let source = match (fields.path, fields.git, fields.tarball) {
            (Some(path), None, None) => {
                if path.as_os_str().is_empty() {
                    return Err(serde::de::Error::custom("`path` must not be empty"));
                }
                ModuleSource::Local { path }
            }
            (None, Some(git), None) => ModuleSource::Git { git },
            (None, None, Some(tarball)) => ModuleSource::Tarball { tarball },
            _ => {
                return Err(serde::de::Error::custom(
                    "choose exactly one of `path`, `git`, or `tarball`",
                ));
            }
        };
        Ok(Self {
            source,
            options: fields.options,
            assets: fields.assets,
        })
    }
}

impl<'de> Deserialize<'de> for GitSource {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            url: String,
            tag: Option<String>,
            branch: Option<String>,
            rev: Option<String>,
            subdir: Option<PathBuf>,
        }
        let fields = Fields::deserialize(deserializer)?;
        Ok(Self {
            url: validate_git(fields.url).map_err(serde::de::Error::custom)?,
            revision: select_revision(fields.tag, fields.branch, fields.rev)
                .map_err(serde::de::Error::custom)?,
            subdir: select_subdir(fields.subdir).map_err(serde::de::Error::custom)?,
        })
    }
}

/// Whether one module directory inside its repository is unset.
fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// Validate a repository address: only an `https` or `ssh` address names a fetchable module.
fn validate_git(git: String) -> std::result::Result<String, &'static str> {
    const ADDRESS: &str = "`git.url` must be an `https` or `ssh` address";
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
                .map_err(|_| "`git.rev` must be a full commit ID")?;
            Ok(Some(RemoteRevision::Rev(commit.to_string())))
        }
        _ => Err("choose at most one of `git.tag`, `git.branch`, or `git.rev`"),
    }
}

/// Validate the module's directory inside its repository.
fn select_subdir(subdir: Option<PathBuf>) -> std::result::Result<PathBuf, &'static str> {
    match subdir {
        Some(path) if is_relative_path(&path) => Ok(path),
        Some(_) => Err("`git.subdir` must be a relative path inside the repository"),
        None => Ok(PathBuf::new()),
    }
}

/// Project selection and replacement of one module's published assets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModuleAssetPolicy {
    pub enable: bool,
    pub replace: BTreeMap<String, PathBuf>,
}

impl Default for ModuleAssetPolicy {
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
    /// The shape of the metadata value returned by this query.
    #[serde(default)]
    pub returns: QueryReturns,
}

/// The supported query result contracts.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QueryReturns {
    #[default]
    Json,
    Entities,
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

/// Whether `path` is a nonempty spelling of plain path segments: no `..`, root, or prefix.
pub fn is_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_declaration_round_trips_its_nested_source() {
        let declaration: ModuleDeclaration = toml::from_str(
            r#"git = { url = "https://example.com/theme.git", subdir = "theme", tag = "v1" }
options = { enabled = true }"#,
        )
        .unwrap();
        let ModuleSource::Git { git } = &declaration.source else {
            panic!("Git source")
        };
        assert_eq!(git.subdir, Path::new("theme"));
        assert_eq!(git.revision, Some(RemoteRevision::Tag("v1".into())));
        let encoded = toml::to_string(&declaration).unwrap();
        assert_eq!(
            toml::from_str::<ModuleDeclaration>(&encoded).unwrap(),
            declaration
        );
    }

    #[test]
    fn git_declarations_reject_mixed_sources_and_selectors() {
        for source in [
            r#"git = "https://example.com/theme.git""#,
            "git = { url = 'https://example.com/theme.git' }\ntag = 'v1'",
            "path = 'theme'\ngit = { url = 'https://example.com/theme.git' }",
            "git = { url = 'https://example.com/theme.git', tag = 'v1', branch = 'main' }",
            "git = { url = 'https://example.com/theme.git', subdir = '../outside' }",
            "git = { url = 'https://example.com/theme.git', rev = 'short' }",
        ] {
            assert!(
                toml::from_str::<ModuleDeclaration>(source).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn resolves_exactly_one_action() {
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
    fn rejects_ambiguous_actions() {
        let ambiguous: Command = toml::from_str("argv = [\"editor\"]\nquery = \"nodes\"").unwrap();
        assert!(ambiguous.action().is_err());

        let projection: Command =
            toml::from_str("argv = [\"editor\"]\nprojection = [\"id\"]").unwrap();
        assert!(projection.action().is_err());
    }

    #[test]
    fn rejects_the_removed_kind_field() {
        assert!(toml::from_str::<Command>("kind = \"argv\"\nargv = [\"editor\"]").is_err());
    }
}
