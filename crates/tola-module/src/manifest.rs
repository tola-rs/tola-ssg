//! Read module manifests and validate their declarations without loading module inputs.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use tola_input::{FilePatterns, InputMode, InputScope, RelativePathError, validate_relative_path};

use crate::action::Action;
use crate::declaration::{
    Assets, ChildModuleDeclaration, Command, CommandAction, CommandActionError, Entry, Initializer,
    ModuleDeclaration, ModuleSource, Parameter, Query, valid_name, valid_scoped_name,
};
use crate::hooks;
use crate::identity::{ModuleCapability, ModuleId, TolaVersion, derive_capabilities};
use crate::module_set::ManifestStamp;

/// Declarations in a module's `tola-module.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    #[serde(default)]
    pub min_tola_version: Option<TolaVersion>,
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
    #[serde(default)]
    pub modules: BTreeMap<String, ChildModuleDeclaration>,
    #[serde(default, deserialize_with = "crate::options::deserialize_options")]
    pub options: toml::Table,
    pub pack: Option<FilePatterns>,
}

impl Manifest {
    /// Capabilities derived from this manifest's declared sections.
    pub fn capabilities(&self) -> Vec<ModuleCapability> {
        derive_capabilities(
            self.build.is_some(),
            self.typst.as_ref(),
            self.nu.is_some(),
            self.assets.as_ref(),
            &self.icons,
            &self.hooks,
            !self.commands.is_empty(),
            !self.queries.is_empty(),
            self.init.is_some(),
            !self.modules.is_empty(),
        )
    }
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
    pub name: ModuleId,
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
    pub evidence: ManifestStamp,
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
    #[error("module `{module}` requires Tola >= {required} (current: {current})")]
    MinTolaVersion {
        module: String,
        required: String,
        current: String,
    },
    #[error("`{child}` is not a usable sub-module name in module `{module}`")]
    ChildModuleName { module: String, child: String },
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

type ManifestDiag<'a> = &'a dyn Fn(String) -> tola_diagnostic::Diagnostic;

fn read_manifest(
    site: &Path,
    name: &str,
    declared_root: PathBuf,
    mode: InputMode,
    sources: &[(PathBuf, Arc<str>)],
    overrides: Option<&toml::Table>,
) -> std::result::Result<ReadManifest, ManifestError> {
    if !valid_scoped_name(name) {
        return Err(ManifestError::Namespace {
            name: name.to_owned(),
        });
    }
    let shown = |path: &Path| tola_diagnostic::display_path(path, site);
    let verified = read_verified_manifest_bytes(site, name, &declared_root, mode, &shown)?;
    let diag = |msg| {
        tola_diagnostic::Diagnostic::at_path(
            tola_diagnostic::codes::module::MANIFEST,
            tola_diagnostic::Severity::Error,
            shown(&verified.manifest_path),
            msg,
        )
        .with_file(verified.canonical_manifest.clone())
    };
    let content = resolve_manifest_text(
        &verified.bytes,
        &verified.canonical_manifest,
        &verified.manifest_path,
        sources,
        &shown,
        &diag,
    )?;
    let digest = blake3::hash(content.as_bytes());
    let mut tbl = parse_manifest_table(&content, &diag)?;
    let (opts, bindings) = bind_manifest_options(&mut tbl, name, overrides, &content, &diag)?;
    let manifest = deserialize_manifest(tbl, &bindings, &content, &diag)?;
    validate_manifest_with_diag(name, &manifest, &diag)?;
    Ok(ReadManifest {
        root: verified.root,
        path: verified.canonical_manifest,
        evidence: ManifestStamp {
            root: verified.root_id,
            manifest: verified.manifest_id,
            digest,
            source: content,
        },
        options: opts,
        manifest,
    })
}

struct VerifiedManifest {
    root: PathBuf,
    canonical_manifest: PathBuf,
    manifest_path: PathBuf,
    root_id: tola_input::FilesystemSourceIdentity,
    manifest_id: tola_input::FilesystemSourceIdentity,
    bytes: Vec<u8>,
}

fn read_verified_manifest_bytes(
    site: &Path,
    name: &str,
    declared_root: &Path,
    mode: InputMode,
    shown: &dyn Fn(&Path) -> String,
) -> Result<VerifiedManifest, ManifestError> {
    use tola_input::FilesystemSourceIdentity;

    let root_id = FilesystemSourceIdentity::from_path(declared_root);
    let root = declared_root
        .canonicalize()
        .map_err(|source| ManifestError::RootPath {
            path: shown(declared_root),
            source,
        })?;
    if !root.is_dir() {
        return Err(ManifestError::RootNotDirectory {
            path: shown(declared_root),
        });
    }
    if root_id.canonical_path() != Some(root.as_path()) {
        return Err(ManifestError::RootChanged {
            path: shown(declared_root),
        });
    }
    if mode.scope() == InputScope::Pure && !root.starts_with(site) {
        return Err(ManifestError::OutsideSiteInPureMode {
            module: name.to_owned(),
        });
    }
    let manifest_path = declared_root.join("tola-module.toml");
    let manifest_id = FilesystemSourceIdentity::from_path(&manifest_path);
    let canonical = manifest_path
        .canonicalize()
        .map_err(|source| ManifestError::ManifestPath {
            file: manifest_path.clone(),
            path: shown(&manifest_path),
            source,
        })?;
    if manifest_id.canonical_path() != Some(canonical.as_path()) {
        return Err(ManifestError::ManifestChanged {
            file: manifest_path.clone(),
            path: shown(&manifest_path),
        });
    }
    if !canonical.starts_with(&root) {
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
    if root_id != FilesystemSourceIdentity::from_path(declared_root)
        || manifest_id != FilesystemSourceIdentity::from_path(&manifest_path)
    {
        return Err(ManifestError::ManifestMoved {
            file: manifest_path,
            path: shown(&declared_root.join("tola-module.toml")),
        });
    }
    Ok(VerifiedManifest {
        root,
        canonical_manifest: canonical,
        manifest_path,
        root_id,
        manifest_id,
        bytes,
    })
}

fn resolve_manifest_text(
    bytes: &[u8],
    canonical_manifest: &Path,
    manifest_path: &Path,
    sources: &[(PathBuf, Arc<str>)],
    shown: &dyn Fn(&Path) -> String,
    diag: ManifestDiag<'_>,
) -> Result<Arc<str>, ManifestError> {
    let mut bufs = sources
        .iter()
        .filter(|(path, _)| path == canonical_manifest);
    if let Some((_, first)) = bufs.next() {
        if bufs.any(|(_, text)| text != first) {
            let msg = "two open copies of `tola-module.toml` contain different text";
            return Err(ManifestError::Rejected {
                diagnostic: Box::new(
                    diag(msg.to_owned())
                        .with_help("Save the version you want to keep, then close the other copy"),
                ),
                source: anyhow::anyhow!(msg),
            });
        }
        return Ok(Arc::clone(first));
    }
    let text = std::str::from_utf8(bytes).map_err(|source| ManifestError::ManifestEncoding {
        file: manifest_path.to_owned(),
        path: shown(manifest_path),
        source,
    })?;
    Ok(Arc::from(text))
}

fn parse_manifest_table(
    content: &Arc<str>,
    diag: ManifestDiag<'_>,
) -> Result<toml::Table, ManifestError> {
    toml::from_str(content).map_err(|err: toml::de::Error| {
        let tola_diagnostic::toml::TomlMessage { message, help } =
            tola_diagnostic::toml::TomlMessage::from_error(err.message());
        let mut d = diag(message.into_owned());
        if let Some(help) = help {
            d = d.with_help(help);
        }
        if let Some(span) = err.span() {
            d = d.with_source_span(content, span);
        }
        ManifestError::Rejected {
            diagnostic: Box::new(d),
            source: err.into(),
        }
    })
}

fn reject_field(
    diag: ManifestDiag<'_>,
    content: &Arc<str>,
    msg: String,
    pointer: &str,
    source: anyhow::Error,
) -> ManifestError {
    ManifestError::Rejected {
        diagnostic: Box::new(locate_field(diag(msg), content, pointer)),
        source,
    }
}

fn bind_manifest_options(
    tbl: &mut toml::Table,
    name: &str,
    overrides: Option<&toml::Table>,
    content: &Arc<str>,
    diag: ManifestDiag<'_>,
) -> Result<(toml::Table, BTreeMap<String, String>), ManifestError> {
    let mut opts = tbl
        .get("options")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    let opt_root = if let Some(overrides) = overrides {
        crate::options::merge(&mut opts, overrides);
        format!("modules.{name}.options")
    } else {
        "options".to_owned()
    };
    crate::options::normalize_options(&mut opts).map_err(|err| {
        let mut ptr = "/options".to_owned();
        crate::fragment::push_pointer_steps(&mut ptr, &err.path);
        reject_field(
            diag,
            content,
            err.to_string(),
            &ptr,
            anyhow::anyhow!(err.to_string()),
        )
    })?;
    let bindings = crate::option_refs::bind(tbl, &opts, &opt_root).map_err(|invalid| {
        let src = anyhow::anyhow!(invalid.reason.clone());
        reject_field(diag, content, invalid.reason, &invalid.pointer, src)
    })?;
    Ok((opts, bindings))
}

fn deserialize_manifest(
    tbl: toml::Table,
    bindings: &BTreeMap<String, String>,
    content: &Arc<str>,
    diag: ManifestDiag<'_>,
) -> Result<Manifest, ManifestError> {
    serde_path_to_error::deserialize(toml::Value::Table(tbl)).map_err(|err| {
        let mut ptr = String::new();
        for seg in err.path() {
            use serde_path_to_error::Segment;
            match seg {
                Segment::Map { key } => tola_diagnostic::push_pointer(&mut ptr, key),
                Segment::Seq { index } => {
                    tola_diagnostic::push_pointer(&mut ptr, &index.to_string())
                }
                Segment::Enum { .. } | Segment::Unknown => {}
            }
        }
        let reason = err.inner().message();
        if let Some(field) = reason
            .strip_prefix("unknown field `")
            .and_then(|s| s.split('`').next())
            && ptr.rsplit('/').next() != Some(field)
        {
            tola_diagnostic::push_pointer(&mut ptr, field);
        }
        let tola_diagnostic::toml::TomlMessage { message, help } =
            tola_diagnostic::toml::TomlMessage::from_error(reason);
        let msg = if let Some((_, opt)) = bindings.iter().find(|(field, _)| {
            ptr.strip_prefix(field.as_str())
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
        }) {
            format!("`{opt}`: {message}")
        } else if message.starts_with("expected ") && !ptr.is_empty() {
            format!("`{}` {message}", err.path())
        } else {
            message.into_owned()
        };
        let mut d = locate_field(diag(msg), content, &ptr);
        if let Some(help) = help {
            d = d.with_help(help);
        }
        ManifestError::Rejected {
            diagnostic: Box::new(d),
            source: err.into(),
        }
    })
}

fn validate_manifest_with_diag(
    name: &str,
    manifest: &Manifest,
    diag: ManifestDiag<'_>,
) -> Result<(), ManifestError> {
    validate_manifest(name, manifest).map_err(|source| {
        let mut d = diag(source.to_string());
        if let ManifestError::CommandUnknownQuery { replacement, .. } = &source {
            d = d.with_help(replacement.as_ref().map_or_else(
                || "Use the name of a query declared in `[queries]`".to_owned(),
                |query| format!("Check whether you meant the query `{query}`"),
            ));
        }
        ManifestError::Rejected {
            diagnostic: Box::new(d),
            source: source.into(),
        }
    })
}

fn locate_field(
    mut diag: tola_diagnostic::Diagnostic,
    content: &Arc<str>,
    mut ptr: &str,
) -> tola_diagnostic::Diagnostic {
    if let Ok(doc) = toml_edit::Document::parse(Arc::clone(content)) {
        let pos = tola_diagnostic::SourcePositions::of(&doc, Arc::clone(content));
        while !ptr.is_empty() {
            if let Some(found) = pos.at_pointer(ptr) {
                diag = diag.with_source_range(found.range());
                if let Some(excerpt) = found.excerpt() {
                    diag = diag.with_source_excerpt(
                        excerpt.line,
                        excerpt.column,
                        excerpt.text,
                        Some(excerpt.highlight),
                    );
                }
                break;
            }
            ptr = ptr.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }
    diag
}

fn validate_manifest(name: &str, manifest: &Manifest) -> Result<(), ManifestError> {
    validate_min_tola_version(name, manifest.min_tola_version.as_ref())?;
    manifest
        .icons
        .validate()
        .map_err(|msg| ManifestError::Source {
            message: format!("module `{name}`: {msg}"),
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
        for (idx, dir) in typst.fonts.iter().enumerate() {
            relative(dir, format!("typst.fonts[{idx}]"))?;
        }
    }
    for (icon_name, src) in &manifest.icons.collections {
        if let Some(path) = src.path() {
            relative(path, format!("icons.collections.{icon_name}.path"))?;
        }
    }
    validate_queries(name, manifest, &relative)?;
    validate_query_parameters(name, manifest)?;
    validate_commands(name, manifest, &relative)?;
    validate_child_modules(name, manifest, &relative)?;
    Ok(())
}

fn validate_min_tola_version(
    name: &str,
    required: Option<&TolaVersion>,
) -> Result<(), ManifestError> {
    let Some(required) = required else {
        return Ok(());
    };
    let current = TolaVersion::current();
    if current < *required {
        return Err(ManifestError::MinTolaVersion {
            module: name.to_owned(),
            required: required.to_string(),
            current: current.to_string(),
        });
    }
    Ok(())
}

fn validate_child_modules(
    name: &str,
    manifest: &Manifest,
    relative: &dyn Fn(&Path, String) -> Result<(), ManifestError>,
) -> Result<(), ManifestError> {
    for (child_name, child) in &manifest.modules {
        if !valid_name(child_name) {
            return Err(ManifestError::ChildModuleName {
                module: name.to_owned(),
                child: child_name.clone(),
            });
        }
        if let ModuleSource::Local { path } = &child.source {
            relative(path, format!("modules.{child_name}.path"))?;
        }
    }
    Ok(())
}

fn validate_queries(
    name: &str,
    manifest: &Manifest,
    relative: &dyn Fn(&Path, String) -> Result<(), ManifestError>,
) -> Result<(), ManifestError> {
    for (query_name, query) in &manifest.queries {
        if !valid_name(query_name) {
            return Err(ManifestError::QueryName {
                module: name.to_owned(),
                query: query_name.clone(),
            });
        }
        let label = format!("{name}.{query_name}");
        if query.label.is_empty() {
            return Err(ManifestError::QueryLabel { query: label });
        }
        relative(&query.entry, format!("query {label} entry"))?;
    }
    Ok(())
}

fn validate_commands(
    name: &str,
    manifest: &Manifest,
    relative: &dyn Fn(&Path, String) -> Result<(), ManifestError>,
) -> Result<(), ManifestError> {
    if manifest.init.is_some() && manifest.commands.contains_key("init") {
        return Err(ManifestError::InitializerDeclaredTwice {
            module: name.to_owned(),
        });
    }
    for (cmd_name, cmd) in &manifest.commands {
        if !valid_name(cmd_name) {
            return Err(ManifestError::CommandName {
                module: name.to_owned(),
                command: cmd_name.clone(),
            });
        }
        let label = format!("{name}.{cmd_name}");
        let action = cmd
            .action()
            .map_err(|source| ManifestError::CommandAction {
                command: label.clone(),
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
                        command: label,
                        query,
                        replacement,
                    });
                }
                if !projection.iter().all(|field| !field.is_empty()) {
                    return Err(ManifestError::CommandProjection { command: label });
                }
            }
            CommandAction::Skill { path } => relative(&path, format!("command {label} skill"))?,
        }
    }
    Ok(())
}

fn validate_query_parameters(module: &str, manifest: &Manifest) -> Result<(), ManifestError> {
    let mut declared = BTreeMap::new();
    for (query_name, query) in &manifest.queries {
        for (kind, params) in [
            ("references", &query.references),
            ("definitions", &query.definitions),
        ] {
            for (idx, param) in params.iter().enumerate() {
                let key = format!("queries.{query_name}.{kind}[{idx}]");
                let invalid = |msg: String| ManifestError::QueryParameter {
                    module: module.to_owned(),
                    key: key.clone(),
                    message: msg,
                };
                if manifest
                    .typst
                    .as_ref()
                    .and_then(|v| v.entry.as_ref())
                    .is_none()
                {
                    return Err(invalid("requires a public `typst.entry`".into()));
                }
                if !param.export.split('.').all(typst::syntax::is_ident) {
                    return Err(invalid("`export` must name a public function".into()));
                }
                if let Parameter::Named { named } = &param.parameter
                    && !typst::syntax::is_ident(named)
                {
                    return Err(invalid(
                        "`parameter.named` must be a Typst identifier".into(),
                    ));
                }
                if let Some(prev) = declared.insert((&param.export, &param.parameter), key.clone())
                {
                    return Err(invalid(format!(
                        "the same function parameter is already declared in `{prev}`"
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
    fn manifest_positions_use_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola-module.toml");
        let src = "[[hooks.before-build]]\nname = 'prepare'\nargv = ['tool']\n";
        std::fs::write(&path, src).unwrap();
        let loaded = read_one(
            dir.path(),
            "module",
            dir.path().to_owned(),
            InputMode::new(InputScope::Online, false),
            &[],
        )
        .unwrap();
        std::fs::write(&path, "\n\n[[hooks.before-build]]\nname = 'changed'\n").unwrap();
        assert_eq!(loaded.evidence.source(), src);
        let pos = loaded.evidence.positions().unwrap();
        assert_eq!(
            pos.written("hooks.before-build.0.name").unwrap().start.line,
            1
        );
        assert!(!loaded.evidence.is_current(&Default::default()).unwrap());
    }

    #[test]
    fn manifest_fifo_nonblocking() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(dir.path().join("tola-module.toml"))
                .status()
                .unwrap()
                .success()
        );
        let res = read_one(
            dir.path(),
            "module",
            dir.path().to_owned(),
            InputMode::new(InputScope::Online, false),
            &[],
        );
        assert!(matches!(
            res,
            Err(ManifestError::ManifestRead { source, .. })
                if source.kind() == std::io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn min_tola_version_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola-module.toml");
        let cur = env!("CARGO_PKG_VERSION");

        std::fs::write(&path, format!("min-tola-version = '{cur}'\n")).unwrap();
        let loaded = read_one(
            dir.path(),
            "docs",
            dir.path().to_owned(),
            InputMode::new(InputScope::Online, false),
            &[],
        )
        .unwrap();
        assert_eq!(
            loaded
                .manifest
                .min_tola_version
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some(cur)
        );

        std::fs::write(&path, "min-tola-version = '999.0.0'\n").unwrap();
        let too_high = read_one(
            dir.path(),
            "docs",
            dir.path().to_owned(),
            InputMode::new(InputScope::Online, false),
            &[],
        );
        assert!(too_high.is_err());

        std::fs::write(&path, "min-tola-version = 'not-a-version'\n").unwrap();
        let invalid = read_one(
            dir.path(),
            "docs",
            dir.path().to_owned(),
            InputMode::new(InputScope::Online, false),
            &[],
        );
        assert!(invalid.is_err());
    }
}
