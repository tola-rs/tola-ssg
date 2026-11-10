//! What the site's modules contribute to one build attempt, and the pure preparation that
//! derives it from declarations and already-acquired module roots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use tola_typst::{BundleCancellation, LocalPackage, LocalPackages};
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot, package::PackageSpec};

use crate::assets::Assets;
use crate::declaration::ModuleDeclaration;
use crate::hooks::{CommandOutput, DevParticipation, HookStage, ModuleHookOrigin, RerunOn};
use crate::module_set::{LoadedModule, load_modules_with_roots};
use crate::watch::HookWatchInputs;
use tola_input::InputMode;

pub fn package_spec(alias: &str) -> Result<PackageSpec> {
    format!("@tola-modules/{alias}:0.0.0")
        .parse()
        .map_err(|error| anyhow::anyhow!("module `{alias}` is not a usable package name: {error}"))
}

pub fn package_path(alias: &str, path: &Path) -> Result<RootedPath> {
    Ok(RootedPath::new(
        VirtualRoot::Package(package_spec(alias)?),
        virtual_path(path)?,
    ))
}

pub fn virtual_path(path: &Path) -> Result<VirtualPath> {
    VirtualPath::new(path.to_string_lossy().replace('\\', "/")).map_err(|error| {
        anyhow::Error::new(error).context(format!(
            "the module source path `{}` is not usable",
            path.display()
        ))
    })
}

/// One module's contribution to the site's static assets.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetContribution {
    pub alias: String,
    pub root: PathBuf,
    pub declarations: Assets,
    pub options: toml::Table,
    pub replace: BTreeMap<String, PathBuf>,
}

/// One module hook, validated and filtered by its `enabled-by` option.
///
/// Neutral to `HooksConfig`: the build maps each entry to its stage's configuration type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEntry {
    pub stage: HookStage,
    /// Execution name, `@module/{alias}/{hook.name}`.
    pub name: String,
    pub origin: ModuleHookOrigin,
    pub action: crate::Action,
    pub dev: DevParticipation,
    /// Extra inputs with `{name}` expanded and roots determined by their scopes.
    pub rerun_on: RerunOn,
    /// Site-relative paths with `{name}` expanded.
    pub generates: Vec<PathBuf>,
    /// `generate-outputs` only, with `{name}` expanded and reservations checked.
    pub outputs: Vec<CommandOutput>,
}

/// Module hooks in lifecycle order: stage, then module alias, then declaration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HooksContribution {
    entries: Vec<HookEntry>,
}

impl HooksContribution {
    pub fn push(&mut self, entry: HookEntry) {
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[HookEntry] {
        &self.entries
    }
}

/// A module-selected compilation entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEntry {
    pub alias: String,
    /// Module root; a logical absolute path that does not follow symlinks.
    pub root: PathBuf,
    /// The manifest's spelling, module-relative; the build joins it to `root`.
    pub entry: PathBuf,
}

/// The entry the contribution selects, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntrySelection {
    Module(ModuleEntry),
    /// A project query file; changes the entry file but not the project's declared entry.
    ProjectFile {
        path: PathBuf,
    },
}

/// Module values one build binds to its compilation and its asset publisher.
#[derive(Debug, Clone)]
pub struct PreparedModules {
    /// Selected modules, keyed by alias.
    pub modules: BTreeMap<String, LoadedModule>,
    /// Local packages bound as `@tola-modules/{alias}:0.0.0`.
    pub packages: Arc<LocalPackages>,
    /// Asset declarations that the build discovers after before-build hooks.
    pub assets: Vec<AssetContribution>,
}

impl PreparedModules {
    /// Recheck the exact manifest bytes and identities that selected this build's modules.
    pub fn manifests_are_fresh(&self, cancellation: &BundleCancellation) -> Result<bool> {
        for module in self.modules.values() {
            let Some(evidence) = &module.manifest_evidence else {
                return Ok(false);
            };
            if !evidence.is_current(cancellation)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Everything modules contribute to one attempt; the build applies it to the project config.
#[derive(Debug, Clone)]
pub struct Contribution {
    pub prepared: PreparedModules,
    pub watch: HookWatchInputs,
}

/// Inputs `prepare` may read other than `Acquired`.
///
/// Preparing is Typst-free: options, hooks, and compilation are the build's later stages.
pub struct Context<'a> {
    pub site_root: &'a Path,
    pub mode: InputMode,
    pub vendor: Option<&'a Path>,
    pub cancellation: &'a BundleCancellation,
}

/// One declared module whose root is read and whose manifest is loaded.
#[derive(Debug, Clone)]
pub struct AcquiredModule {
    pub declaration: ModuleDeclaration,
    pub module: LoadedModule,
}

/// The modules one attempt loaded. Preparing consumes this without touching lock or network.
#[derive(Debug, Clone)]
pub struct Acquired {
    pub modules: BTreeMap<String, AcquiredModule>,
}

/// Read every declared module's root and manifest.
///
/// This is the only step that may read or write `tola.lock` or fetch a remote module.
pub fn acquire(
    ctx: &Context<'_>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
) -> Result<Acquired> {
    let roots = crate::inputs::module_roots_with_bundle(
        ctx.site_root,
        ctx.mode,
        ctx.vendor,
        declarations,
        ctx.cancellation,
    )?;
    let loaded = load_modules_with_roots(ctx.site_root, declarations, &roots, ctx.mode)?;
    let mut modules = BTreeMap::new();
    for (alias, module) in loaded {
        let declaration = declarations
            .get(&alias)
            .with_context(|| format!("module `{alias}` was not declared"))?
            .clone();
        modules.insert(
            alias,
            AcquiredModule {
                declaration,
                module,
            },
        );
    }
    Ok(Acquired { modules })
}

/// Derive one contribution from the declarations and the acquired module roots.
///
/// Pure: no lock write, no cache, no configuration mutation. The same inputs produce the same
/// contribution; every collection is ordered by alias, declaration, or sorted selection.
pub fn prepare(ctx: &Context<'_>, acquired: &Acquired) -> Result<Contribution> {
    let mut packages = Vec::new();
    let mut modules = BTreeMap::new();
    let mut assets = Vec::new();
    for (alias, acquired_module) in &acquired.modules {
        let module = &acquired_module.module;
        // Query-only modules have no public library. Their entry is compiled by FileId.
        let entry = module
            .typst
            .as_ref()
            .and_then(|typst| typst.entry.as_deref())
            .unwrap_or_else(|| Path::new("__tola_no_public_entry.typ"));
        packages.push(LocalPackage::new(
            package_spec(alias)?,
            &module.root,
            virtual_path(entry)?,
        )?);
        if let Some(asset) =
            module_asset_declarations(alias, module, acquired_module.declaration.assets.as_ref())?
        {
            assets.push(asset);
        }
        modules.insert(alias.clone(), module.clone());
    }
    let packages = Arc::new(LocalPackages::new(packages, ["tola-modules"])?);
    Ok(Contribution {
        prepared: PreparedModules {
            modules,
            packages,
            assets,
        },
        watch: watch_inputs(ctx.site_root, acquired)?,
    })
}

/// Conservative development watch paths from every acquired module's declared hooks.
fn watch_inputs(project_root: &Path, acquired: &Acquired) -> Result<HookWatchInputs> {
    let mut inputs = HookWatchInputs::default();
    for acquired_module in acquired.modules.values() {
        crate::hooks::watch_inputs(project_root, &acquired_module.module, &mut inputs)?;
    }
    inputs.hook_scripts.sort_unstable();
    inputs.hook_scripts.dedup();
    inputs.generated_paths.sort_unstable();
    inputs.generated_paths.dedup();
    Ok(inputs)
}

/// A module query entry; the caller has already ensured the module is configured.
pub fn select_query(module: &LoadedModule, query: &crate::declaration::Query) -> EntrySelection {
    EntrySelection::Module(ModuleEntry {
        alias: module.name.clone(),
        root: module.root.clone(),
        entry: query.entry.clone(),
    })
}

/// A project query file; it does not change the project's declared entry.
pub fn select_query_file(entry: &Path) -> EntrySelection {
    EntrySelection::ProjectFile {
        path: entry.to_path_buf(),
    }
}

/// The asset declarations one module contributes. Files are discovered after hooks.
fn module_asset_declarations(
    alias: &str,
    module: &LoadedModule,
    instance: Option<&crate::declaration::ModuleAssetPolicy>,
) -> Result<Option<AssetContribution>> {
    let default = crate::declaration::ModuleAssetPolicy::default();
    let instance = instance.unwrap_or(&default);
    if !instance.enable {
        return Ok(None);
    }
    let Some(declarations) = &module.assets else {
        ensure!(
            instance.replace.is_empty(),
            "module `{alias}` declares no `assets`"
        );
        return Ok(None);
    };
    for (name, source) in &instance.replace {
        tola_address::OutputPath::parse(name).with_context(|| {
            format!("`modules.{alias}.assets.replace` has an invalid asset name `{name}`")
        })?;
        ensure!(
            crate::declaration::is_relative_path(source),
            "`modules.{alias}.assets.replace` must name a path inside the site"
        );
    }
    Ok(Some(AssetContribution {
        alias: alias.to_owned(),
        root: module.root.clone(),
        declarations: declarations.clone(),
        options: module.options.clone(),
        replace: instance.replace.clone(),
    }))
}
