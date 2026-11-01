//! Built-in and configured Nu modules registered under their public namespaces.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NuError, Result};
use nu_protocol::{
    Module, ModuleId,
    engine::{EngineState, StateWorkingSet},
};

/// One site module's public Nu entry, as the CLI resolved it from configuration.
#[derive(Clone, Debug)]
pub struct ConfiguredModule {
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

/// One resolvable `@tola/…` or `@tola-modules/…` entry: its file, its origin, and whether it
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
    pub fn builtin(name: &str) -> Self {
        let root = PathBuf::from("tola").join(name);
        Self {
            alias: name.into(),
            entry: root.join(format!("{name}.nu")),
            root,
            origin: Origin::Builtin { name: name.into() },
        }
    }

    pub fn configured(module: ConfiguredModule, overlays: &BTreeSet<PathBuf>) -> Result<Self> {
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
        let root = fs::canonicalize(&module.root).map_err(|source| NuError::IoStep {
            action: "resolve Nu module root",
            source,
        })?;
        let entry = source_path(&root.join(&module.entry)).map_err(|source| NuError::Context {
            action: "resolve Nu module entry",
            source: Box::new(source),
        })?;
        if !(entry.starts_with(&root) && (entry.is_file() || overlays.contains(&entry))) {
            return Err(NuError::ModuleEntryEscapes);
        }
        let manifest = fs::canonicalize(&module.manifest).map_err(|source| NuError::IoStep {
            action: "resolve Nu module manifest",
            source,
        })?;
        let declaration: toml::Table = fs::read_to_string(&manifest)?.parse()?;
        let declared_entry = declaration
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
            manifest: manifest.clone(),
        };
        Ok(Self {
            alias: module.alias,
            root,
            entry,
            origin,
        })
    }
}

pub(super) fn register(
    engine: &mut EngineState,
    builtins: &[Library],
    configured: &[Library],
    overlays: &[SourceOverlay],
    directories: &mut BTreeSet<PathBuf>,
    observed_sources: &mut BTreeSet<PathBuf>,
) -> Result<BTreeMap<String, ModuleEntry>> {
    let mut entries = BTreeMap::new();
    let mut working = StateWorkingSet::new(engine);
    directories.extend(
        configured
            .iter()
            .map(|module| module.entry.parent().unwrap().to_path_buf())
            .chain(
                overlays
                    .iter()
                    .filter_map(|overlay| overlay.path.parent().map(Path::to_path_buf)),
            ),
    );
    crate::resources::register(&mut working);
    add_overlays(&mut working, overlays, directories)?;
    working.enter_scope();
    for declaration in crate::image::declarations() {
        working.add_decl(declaration);
    }
    for declaration in crate::highlight::declarations() {
        working.add_decl(declaration);
    }
    working.add_decl(Box::new(crate::cli::Capture));
    let mut root = Module::new(b"tola".to_vec());
    for builtin in builtins {
        let id = parse(&mut working, builtin)?;
        if builtin.alias == "tola" {
            root = working.get_module(id).clone();
            continue;
        }
        let main = module_has_main(&working, id);
        root.add_submodule(builtin.alias.as_bytes().to_vec(), id);
        entries.insert(
            format!("@tola/{}", builtin.alias),
            entry(builtin, main, "@tola"),
        );
    }
    working.exit_scope();
    working.add_module("tola", root, Vec::new());
    working.add_module(
        "tola-modules",
        Module::new(b"tola-modules".to_vec()),
        Vec::new(),
    );
    engine.merge_delta(working.render())?;
    // Imports express dependencies themselves. Retry against the growing root without
    // retaining a failed parse's declarations; cycles or invalid exports fail as a group.
    let mut pending: Vec<_> = configured.iter().collect();
    let mut loaded: Vec<(String, ModuleId)> = Vec::new();
    while !pending.is_empty() {
        let before = loaded.len();
        let mut next = Vec::new();
        let mut failures = Vec::new();
        for library in pending {
            let mut candidate = engine.clone();
            let mut working = StateWorkingSet::new(&candidate);
            add_overlays(&mut working, overlays, directories)?;
            working.enter_scope();
            let parsed = parse(&mut working, library);
            for file in working.files() {
                let path = Path::new(file.name.as_ref());
                if path.is_absolute()
                    && let Some(parent) = path.parent()
                {
                    directories.insert(parent.to_path_buf());
                    observed_sources.insert(source_path(path)?);
                }
            }
            match parsed {
                Ok(id) => {
                    let main = module_has_main(&working, id);
                    working.exit_scope();
                    let mut root = Module::new(b"tola-modules".to_vec());
                    for (name, id) in &loaded {
                        root.add_submodule(name.as_bytes().to_vec(), *id);
                    }
                    root.add_submodule(library.alias.as_bytes().to_vec(), id);
                    working.add_module("tola-modules", root, Vec::new());
                    candidate.merge_delta(working.render())?;
                    *engine = candidate;
                    loaded.push((library.alias.clone(), id));
                    entries.insert(
                        format!("@tola-modules/{}", library.alias),
                        entry(library, main, "@tola-modules"),
                    );
                }
                Err(error) => {
                    failures.push(format!("{}:\n{error:#}", library.alias));
                    next.push(library);
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

fn parse(working: &mut StateWorkingSet, library: &Library) -> Result<ModuleId> {
    let path = library
        .entry
        .to_str()
        .ok_or(NuError::NotUtf8 { what: "module" })?;
    // The parser reads this literal as the module path. Naming the module after the configured
    // alias prefixes its public commands with that alias instead of the entry's file stem.
    let literal = nu_utils::escape_quote_string(path);
    let id = nu_parser::parse_module_file_or_dir(
        working,
        literal.as_bytes(),
        nu_protocol::Span::unknown(),
        Some(library.alias.clone()),
    );
    crate::execution::check_errors(working)?;
    let id = id.ok_or(NuError::ModuleUnresolved)?;
    if working.get_module(id).name == library.alias.as_bytes() {
        return Ok(id);
    }
    // The parser caches one module per source span, so a second alias of the same entry answers
    // with the first alias's module. Register that parsed module under this alias too.
    let mut module = working.get_module(id).clone();
    module.name = library.alias.as_bytes().to_vec();
    let comments = working.get_module_comments(id).unwrap_or_default().to_vec();
    Ok(working.add_module(&library.alias, module, comments))
}

fn module_has_main(working: &StateWorkingSet, id: ModuleId) -> bool {
    let module = working.get_module(id);
    module
        .main
        .is_some_and(|id| working.get_decl(id).block_id().is_some())
        || module.decls.iter().any(|(name, id)| {
            name.starts_with(b"main ") && working.get_decl(*id).block_id().is_some()
        })
}

fn entry(library: &Library, has_main: bool, namespace: &str) -> ModuleEntry {
    ModuleEntry {
        path: library.entry.clone(),
        origin: library.origin.clone(),
        locator: format!("{namespace}/{}", library.alias),
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
    let bytes = fs::read(path).map_err(|source| NuError::Input {
        action: "read Nu input",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Dependency {
        path: path.to_path_buf(),
        canonical_path,
        digest: digest(&bytes),
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
    // A buffer may name a source that is not saved yet. Normalize the path as Nu's own path
    // commands do, then resolve it against the nearest existing ancestor.
    let normalized = nu_path::expand_path(path, false);
    let existing = normalized
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or(NuError::SourceIdentity {
            reason: "Nu source has no existing parent",
        })?;
    Ok(fs::canonicalize(existing)?.join(normalized.strip_prefix(existing)?))
}

fn relative_alias(directory: &Path, source: &Path) -> PathBuf {
    let normalized = source_path(directory).unwrap_or_else(|_| directory.to_path_buf());
    let base: Vec<_> = normalized.components().collect();
    let target: Vec<_> = source.components().collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return source.to_path_buf();
    }
    let mut path = directory.to_path_buf();
    for _ in common..base.len() {
        path.push("..");
    }
    for component in &target[common..] {
        path.push(component.as_os_str());
    }
    path
}

/// Register each overlay's text as a file, addressed by its own path and by the alias each
/// directory in `directories` needs to reach it.
fn add_overlays(
    working: &mut StateWorkingSet,
    overlays: &[SourceOverlay],
    directories: &BTreeSet<PathBuf>,
) -> Result<()> {
    for overlay in overlays {
        let path = overlay
            .path
            .to_str()
            .ok_or(NuError::NotUtf8 { what: "overlay" })?;
        let id = working.add_file(path, overlay.text.as_bytes());
        let _ = working.add_virtual_path(path.into(), nu_protocol::engine::VirtualPath::File(id));
        for directory in directories {
            if let Some(alias) = relative_alias(directory, &overlay.path).to_str() {
                let _ = working
                    .add_virtual_path(alias.into(), nu_protocol::engine::VirtualPath::File(id));
            }
        }
    }
    Ok(())
}
