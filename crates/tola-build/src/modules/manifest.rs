//! Read module manifests and validate their declarations without loading module inputs.

use super::{
    Assets, Command, CommandAction, Entry, Initializer, ManifestEvidence, ModuleDeclaration,
    ModuleSource, OptionsSchema, Query, QueryFormat, hooks, is_relative_path, semantics,
    valid_name,
};
use crate::InputScope;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct Manifest {
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) version: Option<tola_typst::PackageVersion>,
    #[serde(default)]
    pub(super) typst: Option<Entry>,
    #[serde(default)]
    pub(super) nu: Option<Entry>,
    #[serde(default)]
    pub(super) site: Option<Entry>,
    #[serde(default)]
    pub(super) options: Option<OptionsSchema>,
    #[serde(default)]
    pub(super) assets: Option<Assets>,
    #[serde(default)]
    pub(super) slots: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub(super) summary: String,
    #[serde(default)]
    pub(super) queries: BTreeMap<String, Query>,
    #[serde(default)]
    pub(super) semantics: Vec<semantics::SemanticRole>,
    #[serde(default)]
    pub(super) commands: BTreeMap<String, Command>,
    #[serde(default)]
    pub(super) init: Option<Initializer>,
    #[serde(default)]
    pub(super) hooks: Vec<hooks::ModuleHook>,
}

/// Statically declared commands, queries, and Nu entry for help discovery.
#[derive(Debug)]
pub struct ModuleDeclarations {
    pub summary: String,
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
}

pub(super) fn read_selected(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    scope: InputScope,
) -> std::result::Result<BTreeMap<String, ReadManifest>, ManifestError> {
    let site = site_root
        .canonicalize()
        .map_err(|source| ManifestError::SiteRoot { source })?;
    declarations
        .iter()
        .map(|(name, declaration)| Ok((name.clone(), read_one(&site, name, declaration, scope)?)))
        .collect()
}

pub fn resolve_nu_modules(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    scope: InputScope,
) -> std::result::Result<BTreeMap<String, NuModule>, ManifestError> {
    read_selected(site_root, declarations, scope)?
        .into_iter()
        .map(|(name, loaded)| {
            let entry = loaded.manifest.nu.map(|nu| nu.entry);
            if let Some(entry) = &entry {
                super::validate_module_path(site_root, &name, &loaded.root, entry, false)?;
            }
            let root = loaded.root;
            Ok((
                name.clone(),
                NuModule {
                    name,
                    manifest: root.join("tola-module.toml"),
                    root,
                    entry,
                },
            ))
        })
        .collect()
}

/// An initializer selected before a destination site exists.
#[derive(Debug)]
pub struct InitializerSource {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub argv: Vec<String>,
}

pub fn resolve_initializer(
    source: &Path,
    alias: Option<&str>,
    scope: InputScope,
) -> std::result::Result<InitializerSource, ManifestError> {
    let root = source
        .canonicalize()
        .map_err(|source| ManifestError::InitializerRoot { source })?;
    let loaded = read_one(
        &root,
        alias.unwrap_or("init"),
        &ModuleDeclaration {
            source: ModuleSource::Local {
                path: PathBuf::from("."),
            },
            ..Default::default()
        },
        scope,
    )?;
    let name = alias
        .map(str::to_owned)
        .or(loaded.manifest.name)
        .ok_or(ManifestError::InitializerName)?;
    let initializer = loaded.manifest.init.ok_or(ManifestError::InitializerArgv)?;
    Ok(InitializerSource {
        name,
        root: loaded.root,
        manifest: loaded.path,
        argv: initializer.argv,
    })
}

pub(super) struct ReadManifest {
    pub root: PathBuf,
    pub path: PathBuf,
    pub evidence: ManifestEvidence,
    pub manifest: Manifest,
}

/// Read only configuration-selected module manifests, without loading their referenced inputs.
pub fn read_declarations(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    scope: InputScope,
) -> std::result::Result<BTreeMap<String, ModuleDeclarations>, ManifestError> {
    read_selected(site_root, declarations, scope)?
        .into_iter()
        .map(|(name, loaded)| {
            let manifest = loaded.manifest;
            let mut commands: BTreeMap<_, _> = manifest
                .commands
                .into_iter()
                .map(|(name, command)| (name, command.help))
                .collect();
            if manifest.init.is_some() {
                commands.insert("init".into(), "Create a site using this module.".into());
            }
            Ok((
                name.clone(),
                ModuleDeclarations {
                    summary: manifest.summary,
                    commands,
                    queries: manifest.queries.into_keys().collect(),
                    nu_entry: manifest.nu.map(|nu| nu.entry),
                },
            ))
        })
        .collect()
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
    #[error("`{name}` is not a usable module name")]
    Namespace { name: String },
    #[error("{message}")]
    Source { message: String },
    #[error("`modules.{module}.path` is empty")]
    EmptyPath { module: String },
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
    #[error("module `{module}` declares unsupported `schema-version` {version}")]
    SchemaVersion { module: String, version: u32 },
    #[error("`{name}` is not a usable package name for module `{module}`")]
    PackageName { module: String, name: String },
    #[error("module `{module}` must declare `name` and `version` together")]
    NameVersionTogether { module: String },
    #[error("module `{module}` needs `name` and `version` to export a Typst, Nu, or site entry")]
    NameVersionForEntry { module: String },
    #[error("`{label}` must be a relative path inside the module")]
    EntryPath { label: String },
    #[error("module `{module}` declares no `options.export`")]
    OptionsExport { module: String },
    #[error("`{slot}` is not a usable slot name in module `{module}`")]
    SlotName { module: String, slot: String },
    #[error("`{query}` is not a usable query name in module `{module}`")]
    QueryName { module: String, query: String },
    #[error("query `{query}` needs a `label`")]
    QueryLabel { query: String },
    #[error("query `{query}` has an invalid `schema-version`")]
    QuerySchemaVersion { query: String },
    #[error("`{query}` must use `schema-version` 1 to return entities")]
    EntityQuerySchemaVersion { query: String },
    #[error("module `{module}` declares invalid semantics: {source}")]
    Semantics {
        module: String,
        #[source]
        source: semantics::SemanticsError,
    },
    #[error("the initializer in module `{module}` needs a command")]
    InitializerExecutable { module: String },
    #[error("module `{module}` declares both `[init]` and `[commands.init]`")]
    InitializerDeclaredTwice { module: String },
    #[error("`{command}` is not a usable command name in module `{module}`")]
    CommandName { module: String, command: String },
    #[error("command `{command}` names no command line")]
    CommandExecutable { command: String },
    #[error("command `{command}` names `{query}`, which the module does not declare")]
    CommandUnknownQuery { command: String, query: String },
    #[error("command `{command}` has an empty projection field")]
    CommandProjection { command: String },
    #[error("Tola could not read the initializer's module directory")]
    InitializerRoot {
        #[source]
        source: std::io::Error,
    },
    #[error("choose the initializer with `--alias`, or declare its `name`")]
    InitializerName,
    #[error("the initializer declares no `argv`")]
    InitializerArgv,
    #[error(transparent)]
    ModulePath(#[from] super::ModulePathError),
}

pub(super) fn read_one(
    site: &Path,
    name: &str,
    declaration: &ModuleDeclaration,
    scope: InputScope,
) -> std::result::Result<ReadManifest, ManifestError> {
    if !valid_name(name) {
        return Err(ManifestError::Namespace {
            name: name.to_owned(),
        });
    }
    let declared_root = match &declaration.source {
        ModuleSource::Local { path } => {
            if path.as_os_str().is_empty() {
                return Err(ManifestError::EmptyPath {
                    module: name.to_owned(),
                });
            }
            site.join(path)
        }
        ModuleSource::Remote { .. } => super::locked::module_root(site, name, &declaration.source)
            .map_err(|_| ManifestError::Source {
                message: format!("module `{name}` is not ready; run `tola module fetch {name}`"),
            })?,
    };
    // Every path this error names reads the way the author wrote it under the site root.
    let shown = |path: &Path| crate::filesystem::display_path(path, site);
    let root_identity = crate::filesystem::FilesystemSourceIdentity::from_path(&declared_root);
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
    if scope == InputScope::Pure && !root.starts_with(site) {
        return Err(ManifestError::OutsideSiteInPureMode {
            module: name.to_owned(),
        });
    }
    let manifest_path = declared_root.join("tola-module.toml");
    let manifest_identity = crate::filesystem::FilesystemSourceIdentity::from_path(&manifest_path);
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
    let bytes = std::fs::read(&manifest_path).map_err(|source| ManifestError::ManifestRead {
        path: shown(&manifest_path),
        source,
    })?;
    if root_identity != crate::filesystem::FilesystemSourceIdentity::from_path(&declared_root)
        || manifest_identity
            != crate::filesystem::FilesystemSourceIdentity::from_path(&manifest_path)
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
    if manifest.schema_version != 1 {
        return Err(ManifestError::SchemaVersion {
            module: name.to_owned(),
            version: manifest.schema_version,
        });
    }
    if let Some(package_name) = &manifest.name
        && !valid_name(package_name)
    {
        return Err(ManifestError::PackageName {
            module: name.to_owned(),
            name: package_name.clone(),
        });
    }
    if manifest.name.is_some() != manifest.version.is_some() {
        return Err(ManifestError::NameVersionTogether {
            module: name.to_owned(),
        });
    }
    if (manifest.typst.is_some() || manifest.nu.is_some() || manifest.site.is_some())
        && manifest.name.is_none()
    {
        return Err(ManifestError::NameVersionForEntry {
            module: name.to_owned(),
        });
    }
    let relative = |path: &Path, label: String| -> std::result::Result<(), ManifestError> {
        if is_relative_path(path) {
            Ok(())
        } else {
            Err(ManifestError::EntryPath { label })
        }
    };
    for (key, entry) in [
        ("typst.entry", manifest.typst.as_ref().map(|v| &v.entry)),
        ("nu.entry", manifest.nu.as_ref().map(|v| &v.entry)),
        ("site.entry", manifest.site.as_ref().map(|v| &v.entry)),
        (
            "options.schema",
            manifest.options.as_ref().map(|v| &v.schema),
        ),
        (
            "assets.directory",
            manifest.assets.as_ref().map(|v| &v.directory),
        ),
    ] {
        if let Some(entry) = entry {
            relative(entry, key.to_owned())?;
        }
    }
    if manifest
        .options
        .as_ref()
        .is_some_and(|options| options.export.trim().is_empty())
    {
        return Err(ManifestError::OptionsExport {
            module: name.to_owned(),
        });
    }
    for (slot, path) in &manifest.slots {
        if !valid_name(slot) {
            return Err(ManifestError::SlotName {
                module: name.to_owned(),
                slot: slot.clone(),
            });
        }
        relative(path, format!("slots.{slot}"))?;
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
        if query.schema_version == 0 {
            return Err(ManifestError::QuerySchemaVersion { query: query_label });
        }
        if query.format == QueryFormat::Entities && query.schema_version != 1 {
            return Err(ManifestError::EntityQuerySchemaVersion { query: query_label });
        }
        relative(&query.entry, format!("query {query_label} entry"))?;
    }
    semantics::validate(
        &manifest.semantics,
        manifest.typst.is_some(),
        &manifest.queries,
    )
    .map_err(|source| ManifestError::Semantics {
        module: name.to_owned(),
        source,
    })?;
    if let Some(initializer) = &manifest.init {
        if !initializer
            .argv
            .first()
            .is_some_and(|program| !program.is_empty())
        {
            return Err(ManifestError::InitializerExecutable {
                module: name.to_owned(),
            });
        }
        if manifest.commands.contains_key("init") {
            return Err(ManifestError::InitializerDeclaredTwice {
                module: name.to_owned(),
            });
        }
    }
    for (command_name, command) in &manifest.commands {
        if !valid_name(command_name) {
            return Err(ManifestError::CommandName {
                module: name.to_owned(),
                command: command_name.clone(),
            });
        }
        let command_label = format!("{name}.{command_name}");
        match &command.action {
            CommandAction::Argv { argv } => {
                if argv.is_empty() || argv[0].is_empty() {
                    return Err(ManifestError::CommandExecutable {
                        command: command_label,
                    });
                }
            }
            CommandAction::Query { query, projection } => {
                if !manifest.queries.contains_key(query) {
                    return Err(ManifestError::CommandUnknownQuery {
                        command: command_label,
                        query: query.clone(),
                    });
                }
                if !projection.iter().all(|field| !field.is_empty()) {
                    return Err(ManifestError::CommandProjection {
                        command: command_label,
                    });
                }
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
            locked_source: match &declaration.source {
                ModuleSource::Remote { .. } => Some((
                    site.to_path_buf(),
                    name.to_owned(),
                    declaration.source.clone(),
                )),
                ModuleSource::Local { .. } => None,
            },
        },
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nu_selection_prepares_no_site_inputs() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("tools.nu"),
            "export def main [] { 42 }\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("tola-module.toml"),
            r#"
schema-version = 1
name = "tools"
version = "1.0.0"
[nu]
entry = "tools.nu"
[typst]
entry = "not-generated.typ"
[assets]
directory = "not-generated-assets"
"#,
        )
        .unwrap();
        let declared = BTreeMap::from([(
            "local".into(),
            ModuleDeclaration {
                source: ModuleSource::Local { path: ".".into() },
                ..Default::default()
            },
        )]);
        let loaded = resolve_nu_modules(directory.path(), &declared, InputScope::Pure).unwrap();
        assert_eq!(
            loaded["local"].entry.as_deref(),
            Some(Path::new("tools.nu"))
        );
        assert!(
            super::super::resolve_modules(directory.path(), &declared, InputScope::Pure).is_err()
        );
    }

    #[test]
    fn nu_must_within_selected_module() {
        let directory = tempfile::tempdir().unwrap();
        let module = directory.path().join("module");
        std::fs::create_dir(&module).unwrap();
        std::fs::write(
            directory.path().join("outside.nu"),
            "export def main [] {}\n",
        )
        .unwrap();
        let declared = BTreeMap::from([(
            "local".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: "module".into(),
                },
                ..Default::default()
            },
        )]);
        for entry in ["", "../outside.nu"] {
            std::fs::write(
                module.join("tola-module.toml"),
                format!("schema-version=1\nname='tools'\nversion='1.0.0'\n[nu]\nentry={entry:?}\n"),
            )
            .unwrap();
            assert!(resolve_nu_modules(directory.path(), &declared, InputScope::Pure).is_err());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../outside.nu", module.join("linked.nu")).unwrap();
            std::fs::write(
                module.join("tola-module.toml"),
                "schema-version=1\nname='tools'\nversion='1.0.0'\n[nu]\nentry='linked.nu'\n",
            )
            .unwrap();
            assert!(resolve_nu_modules(directory.path(), &declared, InputScope::Pure).is_err());
        }
    }

    #[test]
    fn initializer_discovery_loads_nothing() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("tola-module.toml"),
            r#"
schema-version = 1
name = "notebook"
version = "1.0.0"
[site]
entry = "not-created-yet.typ"
[init]
argv = ["not-installed", "{module}/init.nu"]
"#,
        )
        .unwrap();
        let initializer = resolve_initializer(directory.path(), None, InputScope::Pure).unwrap();
        assert_eq!(initializer.name, "notebook");
        assert_eq!(initializer.argv, ["not-installed", "{module}/init.nu"]);
        let initializer =
            resolve_initializer(directory.path(), Some("notes"), InputScope::Pure).unwrap();
        assert_eq!(initializer.name, "notes");
        let declarations = BTreeMap::from([(
            "notes".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: PathBuf::from("."),
                },
                ..Default::default()
            },
        )]);
        let help = read_declarations(directory.path(), &declarations, InputScope::Pure).unwrap();
        assert!(help["notes"].commands.contains_key("init"));
        assert!(
            crate::modules::resolve_modules(directory.path(), &declarations, InputScope::Pure)
                .is_err()
        );
    }
}
