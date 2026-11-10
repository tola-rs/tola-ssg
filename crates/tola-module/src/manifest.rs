//! Read module manifests and validate their declarations without loading module inputs.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tola_input::{FilePatterns, InputMode, InputScope, RelativePathError, validate_relative_path};

use crate::action::Action;
use crate::declaration::{
    Assets, Command, CommandAction, CommandActionError, Entry, Initializer, ModuleDeclaration,
    Parameter, Query, valid_name,
};
use crate::hooks;
use crate::module_set::ManifestEvidence;

/// Declarations in a module's `tola-module.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    #[serde(default)]
    pub typst: Option<Entry>,
    #[serde(default)]
    pub nu: Option<Entry>,
    #[serde(default)]
    pub build: Option<Entry>,
    #[serde(default)]
    pub assets: Option<Assets>,
    #[serde(default)]
    pub queries: BTreeMap<String, Query>,
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
    #[serde(default)]
    pub init: Option<Initializer>,
    #[serde(default)]
    pub hooks: hooks::ModuleHooks,
    #[serde(default, deserialize_with = "crate::options::deserialize_options")]
    pub options: toml::Table,
    pub pack: Option<FilePatterns>,
}

/// Statically declared commands, queries, and Nu entry for help discovery.
#[derive(Debug)]
pub struct ModuleDeclarations {
    pub commands: BTreeMap<String, String>,
    pub queries: Vec<String>,
    pub nu_entry: Option<PathBuf>,
}

/// A configured module's Nu entry, without preparing its Typst or build inputs.
#[derive(Debug)]
pub struct NuModule {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub entry: Option<PathBuf>,
    pub assets_enabled: bool,
    pub options: toml::Table,
}

pub fn read_selected(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    roots: &BTreeMap<String, PathBuf>,
    mode: InputMode,
) -> std::result::Result<BTreeMap<String, ReadManifest>, ManifestError> {
    let site = site_root
        .canonicalize()
        .map_err(|source| ManifestError::SiteRoot { source })?;
    declarations
        .iter()
        .map(|(name, declaration)| {
            let root = roots
                .get(name)
                .cloned()
                .ok_or_else(|| ManifestError::Source {
                    message: format!("module @@{name}@@ was not found; run @@tola update {name}@@"),
                })?;
            let mut loaded = read_one(&site, name, root, mode)?;
            crate::options::merge(&mut loaded.options, &declaration.options);
            Ok((name.clone(), loaded))
        })
        .collect()
}

/// An initializer selected before a destination site exists.
#[derive(Debug)]
pub struct InitializerSource {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub action: Action,
}

pub struct ReadManifest {
    pub root: PathBuf,
    pub path: PathBuf,
    pub evidence: ManifestEvidence,
    pub manifest: Manifest,
    pub options: toml::Table,
}

/// A module manifest Tola could not read, or a declaration it does not accept.
///
/// Every path it names is the site-relative spelling the author wrote it under.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("Tola could not read the site directory")]
    SiteRoot {
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Roots(#[from] crate::inputs::ModuleRootError),
    #[error("`{name}` is not a usable module name")]
    Namespace { name: String },
    #[error("{message}")]
    Source { message: String },
    #[error("Tola could not read the module directory `{path}`")]
    RootPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` is not a directory")]
    RootNotDirectory { path: String },
    #[error("the module directory `{path}` changed while Tola was reading it")]
    RootChanged { path: String },
    #[error("module `{module}` is outside the site, which `--pure` does not allow")]
    OutsideSiteInPureMode { module: String },
    #[error("Tola could not read `{path}`")]
    ManifestPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` changed while Tola was reading it")]
    ManifestChanged { path: String },
    #[error("the manifest `{path}` points outside module `{module}`")]
    ManifestEscapesRoot { module: String, path: String },
    #[error("Tola could not read `{path}`")]
    ManifestRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` changed while Tola was reading it")]
    ManifestMoved { path: String },
    #[error("`{path}` is not UTF-8 text")]
    ManifestEncoding {
        path: String,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("`{path}` is not valid TOML")]
    ManifestParse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("module `{module}` declares an invalid hook: {source}")]
    Hooks {
        module: String,
        #[source]
        source: hooks::HookDeclarationError,
    },
    #[error("module `{module}` declares invalid assets: {source}")]
    Assets {
        module: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("`{label}` in module `{module}` {source}")]
    EntryPath {
        module: String,
        label: String,
        #[source]
        source: RelativePathError,
    },
    #[error("`{query}` is not a usable query name in module `{module}`")]
    QueryName { module: String, query: String },
    #[error("query `{query}` needs a `label`")]
    QueryLabel { query: String },
    #[error("module `{module}` `{key}`: {message}")]
    QueryParameter {
        module: String,
        key: String,
        message: String,
    },
    #[error("module `{module}` declares both `[init]` and `[commands.init]`")]
    InitializerDeclaredTwice { module: String },
    #[error("`{command}` is not a usable command name in module `{module}`")]
    CommandName { module: String, command: String },
    #[error("command `{command}` names `{query}`, which the module does not declare")]
    CommandUnknownQuery { command: String, query: String },
    #[error("command `{command}` has an empty projection field")]
    CommandProjection { command: String },
    #[error("command `{command}`: {source}")]
    CommandAction {
        command: String,
        #[source]
        source: CommandActionError,
    },
    #[error("Tola could not read the initializer's module directory")]
    InitializerRoot {
        #[source]
        source: std::io::Error,
    },
    #[error("choose an alias for the initializer with `--alias`")]
    InitializerName,
    #[error("the module declares no `[init]` action")]
    InitializerMissing,
    #[error(transparent)]
    ModulePath(#[from] crate::module_set::ModulePathError),
}

pub fn read_one(
    site: &Path,
    name: &str,
    declared_root: PathBuf,
    mode: InputMode,
) -> std::result::Result<ReadManifest, ManifestError> {
    if !valid_name(name) {
        return Err(ManifestError::Namespace {
            name: name.to_owned(),
        });
    }

    // Every path this error names reads the way the author wrote it under the site root.
    let shown = |path: &Path| tola_diagnostic::display_path(path, site);
    let root_identity = tola_input::FilesystemSourceIdentity::from_path(&declared_root);
    let root = declared_root
        .canonicalize()
        .map_err(|source| ManifestError::RootPath {
            path: shown(&declared_root),
            source,
        })?;
    if !root.is_dir() {
        return Err(ManifestError::RootNotDirectory {
            path: shown(&declared_root),
        });
    }
    if root_identity.canonical_path() != Some(root.as_path()) {
        return Err(ManifestError::RootChanged {
            path: shown(&declared_root),
        });
    }
    if mode.scope() == InputScope::Pure && !root.starts_with(site) {
        return Err(ManifestError::OutsideSiteInPureMode {
            module: name.to_owned(),
        });
    }
    let manifest_path = declared_root.join("tola-module.toml");
    let manifest_identity = tola_input::FilesystemSourceIdentity::from_path(&manifest_path);
    let canonical_manifest =
        manifest_path
            .canonicalize()
            .map_err(|source| ManifestError::ManifestPath {
                path: shown(&manifest_path),
                source,
            })?;
    if manifest_identity.canonical_path() != Some(canonical_manifest.as_path()) {
        return Err(ManifestError::ManifestChanged {
            path: shown(&manifest_path),
        });
    }
    if !canonical_manifest.starts_with(&root) {
        return Err(ManifestError::ManifestEscapesRoot {
            module: name.to_owned(),
            path: shown(&manifest_path),
        });
    }
    let mut bytes = Vec::new();
    tola_fs::open_regular_file(&manifest_path, true)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|source| ManifestError::ManifestRead {
            path: shown(&manifest_path),
            source,
        })?;
    if root_identity != tola_input::FilesystemSourceIdentity::from_path(&declared_root)
        || manifest_identity != tola_input::FilesystemSourceIdentity::from_path(&manifest_path)
    {
        return Err(ManifestError::ManifestMoved {
            path: shown(&manifest_path),
        });
    }
    let digest = blake3::hash(&bytes);
    let content =
        std::str::from_utf8(&bytes).map_err(|source| ManifestError::ManifestEncoding {
            path: shown(&manifest_path),
            source,
        })?;
    let manifest: Manifest =
        toml::from_str(content).map_err(|source| ManifestError::ManifestParse {
            path: shown(&manifest_path),
            source,
        })?;
    hooks::validate_declarations(&manifest.hooks).map_err(|source| ManifestError::Hooks {
        module: name.to_owned(),
        source,
    })?;
    if let Some(assets) = &manifest.assets {
        assets.validate().map_err(|source| ManifestError::Assets {
            module: name.to_owned(),
            source,
        })?;
    }
    let relative = |path: &Path, label: String| {
        validate_relative_path(path).map_err(|source| ManifestError::EntryPath {
            module: name.to_owned(),
            label,
            source,
        })
    };
    for (key, entry) in [
        ("typst.entry", manifest.typst.as_ref().map(|v| &v.entry)),
        ("nu.entry", manifest.nu.as_ref().map(|v| &v.entry)),
        ("build.entry", manifest.build.as_ref().map(|v| &v.entry)),
    ] {
        if let Some(entry) = entry {
            relative(entry, key.to_owned())?;
        }
    }
    for (query_name, query) in &manifest.queries {
        if !valid_name(query_name) {
            return Err(ManifestError::QueryName {
                module: name.to_owned(),
                query: query_name.clone(),
            });
        }
        let query_label = format!("{name}.{query_name}");
        if query.label.is_empty() {
            return Err(ManifestError::QueryLabel { query: query_label });
        }
        relative(&query.entry, format!("query {query_label} entry"))?;
    }
    validate_query_parameters(name, &manifest)?;
    if manifest.init.is_some() && manifest.commands.contains_key("init") {
        return Err(ManifestError::InitializerDeclaredTwice {
            module: name.to_owned(),
        });
    }
    for (command_name, command) in &manifest.commands {
        if !valid_name(command_name) {
            return Err(ManifestError::CommandName {
                module: name.to_owned(),
                command: command_name.clone(),
            });
        }
        let command_label = format!("{name}.{command_name}");
        let action = command
            .action()
            .map_err(|source| ManifestError::CommandAction {
                command: command_label.clone(),
                source,
            })?;
        match action {
            CommandAction::Execute(_) => {}
            CommandAction::Query { query, projection } => {
                if !manifest.queries.contains_key(&query) {
                    return Err(ManifestError::CommandUnknownQuery {
                        command: command_label,
                        query,
                    });
                }
                if !projection.iter().all(|field| !field.is_empty()) {
                    return Err(ManifestError::CommandProjection {
                        command: command_label,
                    });
                }
            }
            CommandAction::Skill { path } => {
                relative(&path, format!("command {command_label} skill"))?;
            }
        }
    }
    Ok(ReadManifest {
        root,
        path: canonical_manifest,
        evidence: ManifestEvidence {
            root: root_identity,
            manifest: manifest_identity,
            digest,
        },
        options: manifest.options.clone(),
        manifest,
    })
}

fn validate_query_parameters(module: &str, manifest: &Manifest) -> Result<(), ManifestError> {
    let mut declared = BTreeMap::new();
    for (query_name, query) in &manifest.queries {
        for (kind, parameters) in [
            ("references", &query.references),
            ("definitions", &query.definitions),
        ] {
            for (index, parameter) in parameters.iter().enumerate() {
                let key = format!("queries.{query_name}.{kind}[{index}]");
                let invalid = |message: String| ManifestError::QueryParameter {
                    module: module.to_owned(),
                    key: key.clone(),
                    message,
                };
                if manifest.typst.is_none() {
                    return Err(invalid("requires a public `typst.entry`".into()));
                }
                if !parameter.export.split('.').all(typst::syntax::is_ident) {
                    return Err(invalid("`export` must name a public function".into()));
                }
                if let Parameter::Named { named } = &parameter.parameter
                    && !typst::syntax::is_ident(named)
                {
                    return Err(invalid(
                        "`parameter.named` must be a Typst identifier".into(),
                    ));
                }
                if let Some(previous) =
                    declared.insert((&parameter.export, &parameter.parameter), key.clone())
                {
                    return Err(invalid(format!(
                        "the same function parameter is already declared in `{previous}`"
                    )));
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn fifo_manifest_does_not_block() {
        let module = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(module.path().join("tola-module.toml"))
                .status()
                .unwrap()
                .success()
        );
        let result = read_one(
            module.path(),
            "module",
            module.path().to_owned(),
            InputMode::new(InputScope::Online, false),
        );
        assert!(matches!(
            result,
            Err(ManifestError::ManifestRead { source, .. })
                if source.kind() == std::io::ErrorKind::InvalidData
        ));
    }
}
