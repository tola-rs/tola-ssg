//! Loaded module values, path validation, and project-owned module configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tola_input::InputMode;

use crate::declaration::{Assets, Command, Entry, Initializer, ModuleDeclaration, Query};

/// The paths and exact bytes from which a module manifest was parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEvidence {
    pub(crate) root: tola_input::FilesystemSourceIdentity,
    pub(crate) manifest: tola_input::FilesystemSourceIdentity,
    pub(crate) digest: blake3::Hash,
}

impl ManifestEvidence {
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedModule {
    pub name: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
    /// Evidence captured from the exact manifest bytes parsed for this module.
    pub manifest_evidence: Option<ManifestEvidence>,
    pub typst: Option<Entry>,
    pub nu: Option<Entry>,
    pub build: Option<Entry>,
    pub assets: Option<Assets>,
    pub queries: BTreeMap<String, Query>,
    pub semantics: Vec<crate::semantics::SemanticRole>,
    pub commands: BTreeMap<String, Command>,
    pub initializer: Option<Initializer>,
    pub hooks: crate::hooks::ModuleHooks,
    pub options: toml::Table,
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
    #[error(transparent)]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error(transparent)]
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
        crate::manifest::read_selected(site_root, declarations, &roots, mode)?
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
) -> std::result::Result<BTreeMap<String, LoadedModule>, ModuleLoadError> {
    let mut result = BTreeMap::new();
    for (name, loaded) in crate::manifest::read_selected(site_root, declarations, roots, mode)? {
        let root = loaded.root;
        let canonical_manifest = loaded.path;
        let evidence = loaded.evidence;
        let manifest = loaded.manifest;
        result.insert(
            name.clone(),
            LoadedModule {
                name: name.clone(),
                root,
                manifest: canonical_manifest,
                manifest_evidence: Some(evidence),
                typst: manifest.typst,
                nu: manifest.nu,
                build: manifest.build,
                assets: manifest.assets,
                queries: manifest.queries,
                semantics: manifest.semantics,
                commands: manifest.commands,
                initializer: manifest.init,
                hooks: manifest.hooks,
                options: loaded.options,
            },
        );
    }
    Ok(result)
}
