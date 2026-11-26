//! Built-in and configured Nu modules registered under their public namespaces.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NuError, Result};
use crate::host::{HostCommand, Operation};
use nu_protocol::{
    Module, ModuleId,
    engine::{Command, EngineState, StateWorkingSet},
};

pub(super) const NATIVE_MODULE: &str = "tola-native";

/// One site module's public Nu entry, as the CLI resolved it from configuration.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScriptModule {
    pub alias: String,
    pub root: PathBuf,
    pub entry: PathBuf,
    pub manifest: PathBuf,
}

/// Where a module came from: a bundled Tola package, or a configured site module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Builtin {
        name: String,
    },
    Module {
        alias: String,
        root: PathBuf,
        manifest: PathBuf,
    },
}

/// One resolvable `@tola-scripts/…` or `@tola-modules/…` entry: its file, its origin, and whether it
/// exports `main`.
#[derive(Clone, Debug)]
pub struct ModuleEntry {
    pub path: PathBuf,
    pub origin: Origin,
    pub(super) locator: String,
    pub(super) has_main: bool,
}

/// One captured input a session was prepared from: the path it was named by, the target it
/// resolved to, and the digest the session validates against.
#[derive(Clone, Debug)]
pub struct Dependency {
    pub path: PathBuf,
    pub canonical_path: PathBuf,
    pub digest: String,
}

/// One source a module entry read, attributed to the module that owns it.
#[derive(Clone, Debug)]
pub struct SourceOrigin {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub origin: Origin,
    pub digest: String,
    pub version: Option<i64>,
}

/// One in-memory source replacement: the file it stands in for, its text, and the caller's
/// version for that buffer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SourceOverlay {
    pub path: PathBuf,
    pub text: String,
    pub version: i64,
}

pub(super) struct Library {
    pub alias: String,
    pub root: PathBuf,
    pub entry: PathBuf,
    pub origin: Origin,
}

impl Library {
    pub fn builtin(module: &crate::BuiltinModule) -> Self {
        let name = module.name;
        let root = PathBuf::from("tola").join(name);
        Self {
            alias: name.into(),
            entry: root.join(module.entry),
            root,
            origin: Origin::Builtin { name: name.into() },
        }
    }

    pub fn configured(module: ScriptModule, overlays: &BTreeSet<PathBuf>) -> Result<Self> {
        if !(module.root.is_absolute() && module.manifest.is_absolute()) {
            return Err(NuError::ModuleNotAbsolute);
        }
        if module.alias.is_empty() || module.alias.contains(['/', '\\', ':']) {
            return Err(NuError::ModuleAlias {
                alias: module.alias.clone(),
            });
        }
        if !module.entry.is_relative() {
            return Err(NuError::ModuleEntryRelative);
        }
        let root = canonicalize_step(&module.root, "resolve Nu module root")?;
        let entry = source_path(&root.join(&module.entry)).map_err(|source| NuError::Context {
            action: "resolve Nu module entry",
            source: Box::new(source),
        })?;
        if !(entry.starts_with(&root) && (entry.is_file() || overlays.contains(&entry))) {
            return Err(NuError::ModuleEntryEscapes);
        }
        let manifest = canonicalize_step(&module.manifest, "resolve Nu module manifest")?;
        let doc: toml::Table = fs::read_to_string(&manifest)?.parse()?;
        let declared_entry = doc
            .get("nu")
            .and_then(|nu| nu.get("entry"))
            .and_then(toml::Value::as_str)
            .ok_or(NuError::ManifestShape {
                reason: "module has no public Nu entry",
            })?;
        if Path::new(declared_entry) != module.entry {
            return Err(NuError::ModuleEntryChanged {
                alias: module.alias.clone(),
            });
        }
        let origin = Origin::Module {
            alias: module.alias.clone(),
            root: root.clone(),
            manifest,
        };
        Ok(Self {
            alias: module.alias,
            root,
            entry,
            origin,
        })
    }
}

fn canonicalize_step(path: &Path, action: &'static str) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| NuError::IoStep { action, source })
}

pub(super) fn register<'a>(
    engine: &mut EngineState,
    builtins: impl Iterator<Item = &'a crate::BuiltinModule>,
    configured: &[Library],
    overlays: &[SourceOverlay],
    dirs: &mut BTreeSet<PathBuf>,
    observed: &mut BTreeSet<PathBuf>,
    host: &std::sync::Arc<std::sync::OnceLock<crate::Host>>,
) -> Result<(BTreeMap<String, ModuleEntry>, ModuleId)> {
    let mut entries = BTreeMap::new();
    let (native, builtin_entries) =
        register_builtins(engine, builtins, configured, overlays, dirs, host)?;
    entries.extend(builtin_entries);
    let configured_entries = load_configured(engine, configured, overlays, dirs, observed)?;
    entries.extend(configured_entries);
    Ok((entries, native))
}

fn register_builtins<'a>(
    engine: &mut EngineState,
    builtins: impl Iterator<Item = &'a crate::BuiltinModule>,
    configured: &[Library],
    overlays: &[SourceOverlay],
    dirs: &mut BTreeSet<PathBuf>,
    host: &std::sync::Arc<std::sync::OnceLock<crate::Host>>,
) -> Result<(ModuleId, Vec<(String, ModuleEntry)>)> {
    let mut entries = Vec::new();
    let mut ws = StateWorkingSet::new(engine);
    dirs.extend(
        configured
            .iter()
            .map(|m| m.entry.parent().unwrap().to_path_buf())
            .chain(
                overlays
                    .iter()
                    .filter_map(|o| o.path.parent().map(Path::to_path_buf)),
            ),
    );
    add_overlays(&mut ws, overlays, dirs)?;
    ws.enter_scope();
    // Register declarations without leaving their unqualified names in scope.
    ws.enter_scope();
    let mut native = Module::new(NATIVE_MODULE.as_bytes().to_vec());
    for cmd in crate::image::declarations(host.clone()) {
        add_command(&mut ws, &mut native, cmd);
    }
    for op in [Operation::Context, Operation::Render, Operation::Submit] {
        add_command(
            &mut ws,
            &mut native,
            Box::new(HostCommand(op, host.clone())),
        );
    }
    let mut root = Module::new(b"tola".to_vec());
    add_command(&mut ws, &mut root, Box::new(crate::highlight::Highlight));
    for op in [
        Operation::Context,
        Operation::Site,
        Operation::Module,
        Operation::Hook,
        Operation::Assets,
        Operation::Init,
        Operation::Query,
    ] {
        add_command(&mut ws, &mut root, Box::new(HostCommand(op, host.clone())));
    }
    ws.exit_scope();
    // These commands are Custom so Nu's forced-builtin lookup cannot bypass the import.
    let native = ws.add_module(NATIVE_MODULE, native, Vec::new());
    for builtin in builtins {
        let lib = Library::builtin(builtin);
        crate::resources::register(&mut ws, &lib.root, builtin.sources);
        let id = parse(&mut ws, &lib)?;
        let main = module_has_main(&ws, id);
        root.add_submodule(builtin.name.as_bytes().to_vec(), id);
        let entry = entry(&lib, main, "@tola-scripts");
        entries.push((entry.locator.clone(), entry));
    }
    ws.exit_scope();
    ws.add_module("tola", root, Vec::new());
    ws.add_module(
        "tola-modules",
        Module::new(b"tola-modules".to_vec()),
        Vec::new(),
    );
    engine.merge_delta(ws.render())?;
    Ok((native, entries))
}

fn load_configured(
    engine: &mut EngineState,
    configured: &[Library],
    overlays: &[SourceOverlay],
    dirs: &mut BTreeSet<PathBuf>,
    observed: &mut BTreeSet<PathBuf>,
) -> Result<Vec<(String, ModuleEntry)>> {
    // Retry against the growing root without retaining a failed parse's declarations.
    let mut entries = Vec::new();
    let mut pending: Vec<_> = configured.iter().collect();
    let mut loaded: Vec<(String, ModuleId)> = Vec::new();
    while !pending.is_empty() {
        let before = loaded.len();
        let mut next = Vec::new();
        let mut failures = Vec::new();
        for lib in pending {
            let mut candidate = engine.clone();
            let mut ws = StateWorkingSet::new(&candidate);
            add_overlays(&mut ws, overlays, dirs)?;
            ws.enter_scope();
            let parsed = parse(&mut ws, lib);
            for file in ws.files() {
                let path = Path::new(file.name.as_ref());
                if path.is_absolute()
                    && let Some(parent) = path.parent()
                {
                    dirs.insert(parent.to_path_buf());
                    observed.insert(source_path(path)?);
                }
            }
            match parsed {
                Ok(id) => {
                    let main = module_has_main(&ws, id);
                    ws.exit_scope();
                    let mut root = Module::new(b"tola-modules".to_vec());
                    for (name, id) in &loaded {
                        root.add_submodule(name.as_bytes().to_vec(), *id);
                    }
                    root.add_submodule(lib.alias.as_bytes().to_vec(), id);
                    ws.add_module("tola-modules", root, Vec::new());
                    candidate.merge_delta(ws.render())?;
                    *engine = candidate;
                    loaded.push((lib.alias.clone(), id));
                    let entry = entry(lib, main, "@tola-modules");
                    entries.push((entry.locator.clone(), entry));
                }
                Err(err) => {
                    failures.push(format!("{}:\n{err:#}", lib.alias));
                    next.push(lib);
                }
            }
        }
        if !(next.is_empty() || loaded.len() > before) {
            return Err(NuError::ModuleLoad {
                failures: failures.join("\n\n"),
            });
        }
        pending = next;
    }
    Ok(entries)
}

fn add_command(ws: &mut StateWorkingSet, module: &mut Module, cmd: Box<dyn Command>) {
    let name = cmd.name().as_bytes().to_vec();
    module.add_decl(name, ws.add_decl(cmd));
}

fn parse(ws: &mut StateWorkingSet, lib: &Library) -> Result<ModuleId> {
    let path = crate::path_str(&lib.entry, "module")?;
    // Naming after the alias prefixes public commands with the configured alias instead of the file stem.
    let literal = nu_utils::escape_quote_string(path);
    let id = nu_parser::parse_module_file_or_dir(
        ws,
        literal.as_bytes(),
        nu_protocol::Span::unknown(),
        Some(lib.alias.clone()),
    );
    crate::execution::check_errors(ws)?;
    let id = id.ok_or(NuError::ModuleUnresolved)?;
    if ws.get_module(id).name == lib.alias.as_bytes() {
        return Ok(id);
    }
    // The parser caches one module per source span, so a second alias reuses the first module;
    // register that parsed module under this alias too.
    let mut module = ws.get_module(id).clone();
    module.name = lib.alias.as_bytes().to_vec();
    let comments = ws.get_module_comments(id).unwrap_or_default().to_vec();
    Ok(ws.add_module(&lib.alias, module, comments))
}

fn module_has_main(ws: &StateWorkingSet, id: ModuleId) -> bool {
    let module = ws.get_module(id);
    module
        .main
        .is_some_and(|id| ws.get_decl(id).block_id().is_some())
        || module
            .decls
            .iter()
            .any(|(name, id)| name.starts_with(b"main ") && ws.get_decl(*id).block_id().is_some())
}

fn entry(lib: &Library, has_main: bool, namespace: &str) -> ModuleEntry {
    ModuleEntry {
        path: lib.entry.clone(),
        origin: lib.origin.clone(),
        locator: format!("{namespace}/{}", lib.alias),
        has_main,
    }
}

/// Capture an input the caller named by an absolute path, keeping that spelling for diagnostics.
pub(super) fn capture(path: &Path) -> Result<Dependency> {
    if !path.is_absolute() {
        return Err(NuError::SourceIdentity {
            reason: "Nu input path must be absolute",
        });
    }
    let canonical_path = fs::canonicalize(path).map_err(|source| NuError::Input {
        action: "resolve Nu input",
        path: path.to_path_buf(),
        source,
    })?;
    let buf = fs::read(path).map_err(|source| NuError::Input {
        action: "read Nu input",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Dependency {
        path: path.to_path_buf(),
        canonical_path,
        digest: digest(&buf),
    })
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

pub(super) fn source_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(NuError::SourceIdentity {
            reason: "Nu source identity must be absolute",
        });
    }
    if let Ok(path) = fs::canonicalize(path) {
        return Ok(path);
    }
    // Unsaved buffer: normalize without disk access, then resolve from nearest existing ancestor.
    let normalized = nu_path::expand_path(path, false);
    let existing = normalized
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or(NuError::SourceIdentity {
            reason: "Nu source has no existing parent",
        })?;
    Ok(fs::canonicalize(existing)?.join(normalized.strip_prefix(existing)?))
}

fn relative_alias(dir: &Path, src: &Path) -> PathBuf {
    let normalized = source_path(dir).unwrap_or_else(|_| dir.to_path_buf());
    let base: Vec<_> = normalized.components().collect();
    let target: Vec<_> = src.components().collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return src.to_path_buf();
    }
    let mut path = dir.to_path_buf();
    for _ in common..base.len() {
        path.push("..");
    }
    for component in &target[common..] {
        path.push(component.as_os_str());
    }
    path
}

fn add_overlays(
    ws: &mut StateWorkingSet,
    overlays: &[SourceOverlay],
    dirs: &BTreeSet<PathBuf>,
) -> Result<()> {
    for overlay in overlays {
        let path = crate::path_str(&overlay.path, "overlay")?;
        let id = ws.add_file(path, overlay.text.as_bytes());
        let _ = ws.add_virtual_path(path.into(), nu_protocol::engine::VirtualPath::File(id));
        for dir in dirs {
            if let Some(alias) = relative_alias(dir, &overlay.path).to_str() {
                let _ =
                    ws.add_virtual_path(alias.into(), nu_protocol::engine::VirtualPath::File(id));
            }
        }
    }
    Ok(())
}
