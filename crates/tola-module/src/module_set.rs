//! Loaded module values, path validation, and project-owned module configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tola_input::InputMode;

use crate::declaration::{
    Assets, ChildModuleDeclaration, Command, Entry, Initializer, ModuleDeclaration, ModuleSource,
    Query,
};
use crate::identity::{ModuleAlias, ModuleCapability, ModuleId, TolaVersion, derive_capabilities};

/// Who owns a module instance and supplies `owner.root` for its declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ModuleOwner {
    #[default]
    Site,
    Module {
        id: ModuleId,
        root: PathBuf,
    },
}

impl ModuleOwner {
    /// Whether this module was declared directly by the site's `tola.toml`.
    pub fn is_site(&self) -> bool {
        matches!(self, Self::Site)
    }

    /// The owning module's validated identity, or `None` when owned by the site.
    pub fn module_id(&self) -> Option<&ModuleId> {
        match self {
            Self::Site => None,
            Self::Module { id, .. } => Some(id),
        }
    }

    /// The owning module's scoped name, or `None` when owned by the site.
    pub fn module_name(&self) -> Option<&str> {
        self.module_id().map(ModuleId::as_str)
    }

    /// Physical root directory of this module's owner.
    pub fn root<'a>(&'a self, site_root: &'a Path) -> &'a Path {
        match self {
            Self::Site => site_root,
            Self::Module { root, .. } => root.as_path(),
        }
    }
}

/// The paths and exact bytes from which a module manifest was parsed.
#[derive(Debug, Clone)]
pub struct ManifestStamp {
    pub(crate) root: tola_input::FilesystemSourceIdentity,
    pub(crate) manifest: tola_input::FilesystemSourceIdentity,
    pub(crate) digest: blake3::Hash,
    pub(crate) source: std::sync::Arc<str>,
}

impl PartialEq for ManifestStamp {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.manifest == other.manifest && self.digest == other.digest
    }
}

impl Eq for ManifestStamp {}

impl ManifestStamp {
    /// The exact text used to parse this manifest.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Written positions in the manifest snapshot, when it can be parsed for positions.
    pub fn positions(&self) -> Option<tola_diagnostic::SourcePositions> {
        let document = toml_edit::Document::parse(self.source.clone()).ok()?;
        Some(tola_diagnostic::SourcePositions::of(
            &document,
            self.source.clone(),
        ))
    }

    pub fn paths(
        &self,
    ) -> (
        &tola_input::FilesystemSourceIdentity,
        &tola_input::FilesystemSourceIdentity,
    ) {
        (&self.root, &self.manifest)
    }

    pub fn is_current(
        &self,
        cancellation: &tola_typst::BundleCancellation,
    ) -> std::result::Result<bool, tola_typst::CompileError> {
        cancellation.ensure_active()?;
        let identity = || {
            (
                tola_input::FilesystemSourceIdentity::from_path(self.root.logical_path()),
                tola_input::FilesystemSourceIdentity::from_path(self.manifest.logical_path()),
            )
        };
        if identity() != (self.root.clone(), self.manifest.clone()) {
            return Ok(false);
        }
        let mut hasher = blake3::Hasher::new();
        match tola_input::read_file_chunks(
            self.manifest.logical_path(),
            cancellation.flag(),
            |bytes| {
                hasher.update(bytes);
            },
        ) {
            Ok(()) => {}
            Err(tola_input::FileReadError::Cancelled(_)) => {
                return Err(tola_typst::CompileError::Cancelled);
            }
            Err(tola_input::FileReadError::Io(_)) => return Ok(false),
        }
        cancellation.ensure_active()?;
        Ok(hasher.finalize() == self.digest
            && identity() == (self.root.clone(), self.manifest.clone()))
    }
}

/// A module and the manifest that supplied its declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModule {
    pub name: ModuleId,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub owner: ModuleOwner,
    pub source: Option<ModuleSource>,
    /// Evidence captured from the exact manifest bytes parsed for this module.
    pub manifest_evidence: Option<ManifestStamp>,
    pub min_tola_version: Option<TolaVersion>,
    pub typst: Option<crate::manifest::TypstInputs>,
    pub nu: Option<Entry>,
    pub build: Option<Entry>,
    pub assets: Option<Assets>,
    pub icons: crate::icons::Icons,
    pub queries: BTreeMap<String, Query>,
    pub commands: BTreeMap<String, Command>,
    pub initializer: Option<Initializer>,
    pub hooks: crate::hooks::ModuleHooks,
    pub modules: BTreeMap<String, ChildModuleDeclaration>,
    pub child_aliases: BTreeMap<ModuleAlias, ModuleId>,
    pub options: toml::Table,
}

impl LoadedModule {
    /// Capabilities derived from this module's declared sections.
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
            self.initializer.is_some(),
            !self.modules.is_empty(),
        )
    }
}

/// A module path Tola could not read, or one that leaves its module.
#[derive(Debug, thiserror::Error)]
pub enum ModulePathError {
    #[error("Tola could not read `{path}`")]
    Unreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` is outside module `{module}`")]
    Outside { module: String, path: String },
    #[error("`{path}` is not a file")]
    NotAFile { path: String },
    #[error("`{path}` is not a directory")]
    NotADirectory { path: String },
}

/// Why one declared module path is unusable.
#[derive(Debug, thiserror::Error)]
pub enum ModuleLoadError {
    #[error("{0}")]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error("{0}")]
    Roots(#[from] crate::inputs::ModuleRootError),
}

pub fn validate_module_path(
    site_root: &Path,
    module: &str,
    root: &Path,
    path: &Path,
    directory: bool,
) -> std::result::Result<PathBuf, ModulePathError> {
    let shown = tola_diagnostic::display_path(&root.join(path), site_root);
    let path = root
        .join(path)
        .canonicalize()
        .map_err(|source| ModulePathError::Unreadable {
            path: shown.clone(),
            source,
        })?;
    if !path.starts_with(root) {
        return Err(ModulePathError::Outside {
            module: module.to_owned(),
            path: shown,
        });
    }
    let right_type = if directory {
        path.is_dir()
    } else {
        path.is_file()
    };
    if !right_type {
        return Err(if directory {
            ModulePathError::NotADirectory { path: shown }
        } else {
            ModulePathError::NotAFile { path: shown }
        });
    }
    Ok(path)
}

/// Commands and initializer rooted in one module, independent of its build inputs.
#[derive(Debug)]
pub struct ModuleCommands {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub commands: BTreeMap<String, Command>,
    pub initializer: Option<Initializer>,
    pub options: toml::Table,
}

/// Command declarations remain available when unrelated build inputs need repair.
pub fn select_command_modules(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    mode: InputMode,
    cancellation: &tola_typst::BundleCancellation,
) -> std::result::Result<BTreeMap<String, ModuleCommands>, crate::manifest::ManifestError> {
    let roots = crate::inputs::module_roots_with_bundle(
        site_root,
        mode,
        vendor,
        declarations,
        cancellation,
    )?;
    Ok(
        crate::manifest::read_selected(site_root, declarations, &roots, mode, &[])?
            .into_iter()
            .map(|(name, loaded)| {
                (
                    name.clone(),
                    ModuleCommands {
                        name,
                        root: loaded.root,
                        manifest: loaded.path,
                        commands: loaded.manifest.commands,
                        initializer: loaded.manifest.init,
                        options: loaded.options,
                    },
                )
            })
            .collect(),
    )
}

/// Read every declared module's manifest through a read root.
pub fn load_modules_with_roots(
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    roots: &BTreeMap<String, PathBuf>,
    mode: InputMode,
    sources: &[(PathBuf, std::sync::Arc<str>)],
) -> std::result::Result<BTreeMap<String, LoadedModule>, ModuleLoadError> {
    crate::manifest::read_selected(site_root, declarations, roots, mode, sources)?
        .into_iter()
        .map(|(name, loaded)| {
            let id = ModuleId::parse(&name)
                .map_err(|_| crate::manifest::ManifestError::Namespace { name: name.clone() })?;
            let source = declarations.get(&name).map(|d| d.source.clone());
            let manifest = loaded.manifest;
            Ok((
                name,
                LoadedModule {
                    name: id,
                    root: loaded.root,
                    manifest: loaded.path,
                    owner: ModuleOwner::Site,
                    source,
                    manifest_evidence: Some(loaded.evidence),
                    min_tola_version: manifest.min_tola_version,
                    typst: manifest.typst,
                    nu: manifest.nu,
                    build: manifest.build,
                    assets: manifest.assets,
                    icons: manifest.icons,
                    queries: manifest.queries,
                    commands: manifest.commands,
                    initializer: manifest.init,
                    hooks: manifest.hooks,
                    modules: manifest.modules,
                    child_aliases: BTreeMap::new(),
                    options: loaded.options,
                },
            ))
        })
        .collect()
}
