//! Read module manifests and validate their declarations without loading module inputs.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use tola_input::{FilePatterns, InputMode, InputScope, RelativePathError, validate_relative_path};

use crate::action::Action;
use crate::declaration::{
    Assets, Command, CommandAction, CommandActionError, Entry, Initializer, ModuleDeclaration,
    ModuleSource, Parameter, Query, valid_name,
};
use crate::hooks;
use crate::module_set::ManifestEvidence;

/// Declarations in a module's `tola-module.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    #[serde(default)]
    pub typst: Option<TypstInputs>,
    #[serde(default)]
    pub nu: Option<Entry>,
    #[serde(default)]
    pub build: Option<Entry>,
    #[serde(default)]
    pub assets: Option<Assets>,
    #[serde(default)]
    pub icons: crate::icons::Icons,
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

/// Typst library entry and compiler fonts supplied by a module.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TypstInputs {
    /// Public library entry relative to the module root.
    pub entry: Option<PathBuf>,
    /// Compiler font directories relative to the module root.
    #[serde(default)]
    pub fonts: Vec<PathBuf>,
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
    /// Font directories declared by the module, relative to its root.
    pub font_directories: Vec<PathBuf>,
    pub assets_enabled: bool,
    pub options: toml::Table,
}

pub fn read_selected(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    roots: &BTreeMap<String, PathBuf>,
    mode: InputMode,
    sources: &[(PathBuf, Arc<str>)],
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
            let sources = match &declaration.source {
                ModuleSource::Local { .. } => sources,
                _ => &[],
            };
            let loaded = read_with_options(&site, name, root, mode, sources, &declaration.options)?;
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
        file: PathBuf,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` changed while Tola was reading it")]
    ManifestChanged { file: PathBuf, path: String },
    #[error("`{path}` points outside module `{module}`")]
    ManifestEscapesRoot {
        file: PathBuf,
        module: String,
        path: String,
    },
    #[error("Tola could not read `{path}`")]
    ManifestRead {
        file: PathBuf,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` changed while Tola was reading it")]
    ManifestMoved { file: PathBuf, path: String },
    #[error("`{path}` is not UTF-8 text")]
    ManifestEncoding {
        file: PathBuf,
        path: String,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("{}", .diagnostic.message)]
    Rejected {
        diagnostic: Box<tola_diagnostic::Diagnostic>,
        #[source]
        source: anyhow::Error,
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
    #[error("command `{command}` names an unknown query `{query}`")]
    CommandUnknownQuery {
        command: String,
        query: String,
        replacement: Option<String>,
    },
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
    sources: &[(PathBuf, Arc<str>)],
) -> std::result::Result<ReadManifest, ManifestError> {
    read_manifest(site, name, declared_root, mode, sources, None)
}

/// Read a configured module using its instance options before checking referenced fields.
pub fn read_with_options(
    site: &Path,
    name: &str,
    declared_root: PathBuf,
    mode: InputMode,
    sources: &[(PathBuf, Arc<str>)],
    options: &toml::Table,
) -> std::result::Result<ReadManifest, ManifestError> {
    read_manifest(site, name, declared_root, mode, sources, Some(options))
}

fn read_manifest(
    site: &Path,
    name: &str,
    declared_root: PathBuf,
    mode: InputMode,
    sources: &[(PathBuf, Arc<str>)],
    overrides: Option<&toml::Table>,
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
                file: manifest_path.clone(),
                path: shown(&manifest_path),
                source,
            })?;
    if manifest_identity.canonical_path() != Some(canonical_manifest.as_path()) {
        return Err(ManifestError::ManifestChanged {
            file: manifest_path.clone(),
            path: shown(&manifest_path),
        });
    }
    if !canonical_manifest.starts_with(&root) {
        return Err(ManifestError::ManifestEscapesRoot {
            file: manifest_path.clone(),
            module: name.to_owned(),
            path: shown(&manifest_path),
        });
    }
    let mut bytes = Vec::new();
    tola_fs::open_regular_file(&manifest_path, true)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|source| ManifestError::ManifestRead {
            file: manifest_path.clone(),
            path: shown(&manifest_path),
            source,
        })?;
    if root_identity != tola_input::FilesystemSourceIdentity::from_path(&declared_root)
        || manifest_identity != tola_input::FilesystemSourceIdentity::from_path(&manifest_path)
    {
        return Err(ManifestError::ManifestMoved {
            file: manifest_path.clone(),
            path: shown(&manifest_path),
        });
    }
    let diagnostic = |message| {
        tola_diagnostic::Diagnostic::at_path(
            tola_diagnostic::codes::module::MANIFEST,
            tola_diagnostic::Severity::Error,
            shown(&manifest_path),
            message,
        )
        .with_file(canonical_manifest.clone())
    };
    let overlay = || {
        let mut buffers = sources
            .iter()
            .filter(|(path, _)| path == &canonical_manifest);
        let Some((_, first)) = buffers.next() else {
            return Ok(None);
        };
        if buffers.any(|(_, text)| text != first) {
            let message = "two open copies of `tola-module.toml` contain different text";
            return Err(ManifestError::Rejected {
                diagnostic: Box::new(
                    diagnostic(message.to_owned())
                        .with_help("Save the version you want to keep, then close the other copy"),
                ),
                source: anyhow::anyhow!(message),
            });
        }
        Ok(Some(first.as_ref()))
    };
    let content = match overlay()? {
        Some(text) => text,
        None => std::str::from_utf8(&bytes).map_err(|source| ManifestError::ManifestEncoding {
            file: manifest_path.clone(),
            path: shown(&manifest_path),
            source,
        })?,
    };
    let content: Arc<str> = Arc::from(content);
    let digest = blake3::hash(content.as_bytes());
    let mut table: toml::Table = toml::from_str(&content).map_err(|source: toml::de::Error| {
        let tola_diagnostic::toml::TomlMessage { message, help } =
            tola_diagnostic::toml::TomlMessage::from_error(source.message());
        let mut diagnostic = diagnostic(message.into_owned());
        if let Some(help) = help {
            diagnostic = diagnostic.with_help(help);
        }
        if let Some(span) = source.span() {
            diagnostic = diagnostic.with_source_span(&content, span);
        }
        ManifestError::Rejected {
            diagnostic: Box::new(diagnostic),
            source: source.into(),
        }
    })?;
    let mut options = table
        .get("options")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    let option_root = if let Some(overrides) = overrides {
        crate::options::merge(&mut options, overrides);
        format!("modules.{name}.options")
    } else {
        "options".to_owned()
    };
    let reject_field = |message: String, pointer: &str, source: anyhow::Error| {
        let diagnostic = locate_field(diagnostic(message), &content, pointer);
        ManifestError::Rejected {
            diagnostic: Box::new(diagnostic),
            source,
        }
    };
    crate::options::normalize_options(&mut options).map_err(|error| {
        let mut pointer = "/options".to_owned();
        for step in &error.path {
            match step {
                crate::fragment::FragmentStep::Key(key) => {
                    tola_diagnostic::push_pointer(&mut pointer, key);
                }
                crate::fragment::FragmentStep::Index(index) => {
                    tola_diagnostic::push_pointer(&mut pointer, &index.to_string());
                }
            }
        }
        reject_field(
            error.to_string(),
            &pointer,
            anyhow::anyhow!(error.to_string()),
        )
    })?;
    let bindings =
        crate::option_refs::bind(&mut table, &options, &option_root).map_err(|invalid| {
            let source = anyhow::anyhow!(invalid.reason.clone());
            reject_field(invalid.reason, &invalid.field_pointer, source)
        })?;
    let manifest: Manifest =
        serde_path_to_error::deserialize(toml::Value::Table(table)).map_err(|error| {
            let mut pointer = String::new();
            for segment in error.path() {
                use serde_path_to_error::Segment;
                match segment {
                    Segment::Map { key } => tola_diagnostic::push_pointer(&mut pointer, key),
                    Segment::Seq { index } => {
                        tola_diagnostic::push_pointer(&mut pointer, &index.to_string());
                    }
                    Segment::Enum { .. } | Segment::Unknown => {}
                }
            }
            let reason = error.inner().message();
            if let Some(field) = reason
                .strip_prefix("unknown field `")
                .and_then(|reason| reason.split('`').next())
                && pointer.rsplit('/').next() != Some(field)
            {
                tola_diagnostic::push_pointer(&mut pointer, field);
            }
            let tola_diagnostic::toml::TomlMessage { message, help } =
                tola_diagnostic::toml::TomlMessage::from_error(reason);
            let message = if let Some((_, option)) = bindings.iter().find(|(field, _)| {
                pointer
                    .strip_prefix(field.as_str())
                    .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
            }) {
                format!("`{option}`: {message}")
            } else if message.starts_with("expected ") && !pointer.is_empty() {
                format!("`{}` {message}", error.path())
            } else {
                message.into_owned()
            };
            let mut diagnostic = locate_field(diagnostic(message), &content, &pointer);
            if let Some(help) = help {
                diagnostic = diagnostic.with_help(help);
            }
            ManifestError::Rejected {
                diagnostic: Box::new(diagnostic),
                source: error.into(),
            }
        })?;
    validate_manifest(name, &manifest).map_err(|source| {
        let mut diagnostic = diagnostic(source.to_string());
        if let ManifestError::CommandUnknownQuery { replacement, .. } = &source {
            diagnostic = diagnostic.with_help(replacement.as_ref().map_or_else(
                || "Use the name of a query declared in `[queries]`".to_owned(),
                |query| format!("Check whether you meant the query `{query}`"),
            ));
        }
        ManifestError::Rejected {
            diagnostic: Box::new(diagnostic),
            source: source.into(),
        }
    })?;
    Ok(ReadManifest {
        root,
        path: canonical_manifest,
        evidence: ManifestEvidence {
            root: root_identity,
            manifest: manifest_identity,
            digest,
            source: content,
        },
        options,
        manifest,
    })
}

fn locate_field(
    mut diagnostic: tola_diagnostic::Diagnostic,
    content: &Arc<str>,
    mut pointer: &str,
) -> tola_diagnostic::Diagnostic {
    if let Ok(document) = toml_edit::Document::parse(Arc::clone(content)) {
        let positions = tola_diagnostic::SourcePositions::of(&document, Arc::clone(content));
        while !pointer.is_empty() {
            if let Some(position) = positions.at_pointer(pointer) {
                diagnostic = diagnostic.with_source_range(position.range());
                if let Some(excerpt) = position.excerpt() {
                    diagnostic = diagnostic.with_source_excerpt(
                        excerpt.line,
                        excerpt.column,
                        excerpt.text,
                        Some(excerpt.highlight),
                    );
                }
                break;
            }
            pointer = pointer.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }
    diagnostic
}

fn validate_manifest(name: &str, manifest: &Manifest) -> Result<(), ManifestError> {
    manifest
        .icons
        .validate()
        .map_err(|message| ManifestError::Source {
            message: format!("module `{name}`: {message}"),
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
        (
            "typst.entry",
            manifest.typst.as_ref().and_then(|v| v.entry.as_ref()),
        ),
        ("nu.entry", manifest.nu.as_ref().map(|v| &v.entry)),
        ("build.entry", manifest.build.as_ref().map(|v| &v.entry)),
    ] {
        if let Some(entry) = entry {
            relative(entry, key.to_owned())?;
        }
    }
    if let Some(typst) = &manifest.typst {
        for (index, directory) in typst.fonts.iter().enumerate() {
            relative(directory, format!("typst.fonts[{index}]"))?;
        }
    }
    for (name, source) in &manifest.icons.collections {
        if let Some(path) = source.path() {
            relative(path, format!("icons.collections.{name}.path"))?;
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
    validate_query_parameters(name, manifest)?;
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
                    let replacement = tola_typst::spelling::closest(
                        &query,
                        manifest.queries.keys().map(String::as_str),
                    )
                    .map(str::to_owned);
                    return Err(ManifestError::CommandUnknownQuery {
                        command: command_label,
                        query,
                        replacement,
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
    Ok(())
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
                if manifest
                    .typst
                    .as_ref()
                    .and_then(|v| v.entry.as_ref())
                    .is_none()
                {
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
    fn manifest_positions_use_the_loaded_snapshot() {
        let module = tempfile::tempdir().unwrap();
        let path = module.path().join("tola-module.toml");
        let source = "[[hooks.before-build]]\nname = 'prepare'\nargv = ['tool']\n";
        std::fs::write(&path, source).unwrap();
        let loaded = read_one(
            module.path(),
            "module",
            module.path().to_owned(),
            InputMode::new(InputScope::Online, false),
            &[],
        )
        .unwrap();
        std::fs::write(&path, "\n\n[[hooks.before-build]]\nname = 'changed'\n").unwrap();
        assert_eq!(loaded.evidence.source(), source);
        let positions = loaded.evidence.positions().unwrap();
        assert_eq!(
            positions
                .written("hooks.before-build.0.name")
                .unwrap()
                .start
                .line,
            1
        );
        assert!(!loaded.evidence.is_current(&Default::default()).unwrap());
    }

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
            &[],
        );
        assert!(matches!(
            result,
            Err(ManifestError::ManifestRead { source, .. })
                if source.kind() == std::io::ErrorKind::InvalidData
        ));
    }
}
