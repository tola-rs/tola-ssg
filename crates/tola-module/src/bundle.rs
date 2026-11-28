//! What the site's modules contribute to one build attempt, and the pure preparation that
//! derives it from declarations and already-acquired module roots.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use tola_input::{InputLock, LockedModule};
use tola_typst::{BundleCancellation, LocalPackage, LocalPackages};
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot, package::PackageSpec};

use crate::assets::Assets;
use crate::declaration::{ModuleDeclaration, ModuleSource};
use crate::hooks::{CommandOutput, DevParticipation, HookStage, ModuleHookOrigin, RerunOn};
use crate::identity::{ModuleAlias, ModuleId};
use crate::manifest::ManifestError;
use crate::module_set::{LoadedModule, ModuleOwner};
use crate::watch::HookWatchInputs;
use tola_input::InputMode;

pub fn package_spec(alias: &str) -> Result<PackageSpec> {
    package_spec_in("tola-modules", alias)
}

pub fn dep_package_spec(alias: &str) -> Result<PackageSpec> {
    package_spec_in("tola-deps", alias)
}

pub fn dep_package_path(alias: &str, path: &Path) -> Result<RootedPath> {
    Ok(RootedPath::new(
        VirtualRoot::Package(dep_package_spec(alias)?),
        virtual_path(path)?,
    ))
}

fn package_spec_in(namespace: &str, alias: &str) -> Result<PackageSpec> {
    format!("@{namespace}/{alias}:0.0.0")
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

/// Why a module hook participates in a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookParticipation {
    Enabled,
    Disabled,
    OptionDisabled,
}

/// One module hook, validated against its instance options.
///
/// Disabled declarations remain present for identity checks and source locations; consumers
/// use `participation` to select execution rather than dropping entries during collection.
/// Neutral to `HooksConfig`: the build maps each entry to its stage's configuration type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEntry {
    pub stage: HookStage,
    /// Execution name, `@module/{alias}/{hook.name}`.
    pub name: String,
    /// Name written in the module manifest.
    pub declared_name: String,
    /// Batch number local to this module instance and stage, not a cross-module priority.
    pub order: u64,
    /// Whether the declaration is enabled by its own fields and instance options.
    pub participation: HookParticipation,
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

/// Module declarations collected by alias, then stage and source declaration order.
/// The build separates stages and applies each instance's order batches afterward.
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
pub struct ModuleBundle {
    /// Selected modules, keyed by alias.
    pub modules: BTreeMap<String, LoadedModule>,
    /// Local packages bound as `@tola-modules/{alias}:0.0.0`.
    pub packages: Arc<LocalPackages>,
    /// Asset declarations that the build discovers after before-build hooks.
    pub assets: Vec<AssetContribution>,
}

impl ModuleBundle {
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
    pub prepared: ModuleBundle,
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
    pub sources: &'a [(PathBuf, std::sync::Arc<str>)],
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

/// Read every declared module's root and manifest, including private `[modules.*]` children.
///
/// This is the only step that may read or write `tola.lock` or fetch a remote module.
pub fn acquire(
    ctx: &Context<'_>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
) -> Result<Acquired> {
    let site_root = ctx
        .site_root
        .canonicalize()
        .map_err(|source| crate::inputs::ModuleRootError::SiteRoot { source })?;
    let mut lock = InputLock::read(&site_root)?.unwrap_or_default();
    let original = lock.clone();
    let acquired = acquire_with_lock(ctx, &site_root, declarations, &mut lock)?;
    if lock != original && ctx.mode.may_write_lock() {
        ctx.cancellation.ensure_active()?;
        lock.commit(&site_root, &original)?;
    }
    Ok(acquired)
}

/// Read every declared module's root and manifest using an existing lock table in memory.
pub fn acquire_with_lock(
    ctx: &Context<'_>,
    site_root: &Path,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    lock: &mut InputLock,
) -> Result<Acquired> {
    let mut acquired = BTreeMap::new();
    let mut ancestors = Vec::new();
    for (alias, declaration) in declarations {
        let local_alias = ModuleAlias::parse(alias)?;
        let scoped_id = ModuleId::top_level(local_alias.clone());
        let target = crate::inputs::AcquireTarget {
            scoped_alias: &scoped_id,
            lock_key: &local_alias,
            owner_root: site_root,
        };
        acquire_node(
            ctx,
            site_root,
            &target,
            ModuleOwner::Site,
            declaration,
            &mut lock.modules,
            &mut ancestors,
            &mut acquired,
        )?;
    }
    Ok(Acquired { modules: acquired })
}

#[allow(clippy::too_many_arguments)]
fn acquire_node(
    ctx: &Context<'_>,
    site_root: &Path,
    target: &crate::inputs::AcquireTarget<'_>,
    owner: ModuleOwner,
    declaration: &ModuleDeclaration,
    parent_lock: &mut BTreeMap<String, LockedModule>,
    ancestors: &mut Vec<(PathBuf, ModuleId)>,
    out: &mut BTreeMap<String, AcquiredModule>,
) -> Result<()> {
    ctx.cancellation.ensure_active()?;
    let needs_lock = !target.is_child()
        || matches!(
            declaration.source,
            ModuleSource::Git { .. } | ModuleSource::Tarball { .. } | ModuleSource::Local { .. }
        );
    let root = crate::inputs::acquire_one(
        site_root,
        ctx.mode,
        ctx.vendor,
        target,
        declaration,
        needs_lock.then_some(&mut *parent_lock),
        ctx.cancellation,
    )?;
    let canonical = root
        .canonicalize()
        .map_err(|source| ManifestError::RootPath {
            path: tola_diagnostic::display_path(&root, site_root),
            source,
        })?;
    if let Some(pos) = ancestors.iter().position(|(path, _)| path == &canonical) {
        let mut chain: Vec<_> = ancestors[pos..]
            .iter()
            .map(|(_, id)| id.as_str().to_owned())
            .collect();
        chain.push(target.scoped_alias.as_str().to_owned());
        return Err(crate::inputs::ModuleRootError::Cycle { chain }.into());
    }
    let sources = if matches!(declaration.source, ModuleSource::Local { .. }) {
        ctx.sources
    } else {
        &[]
    };
    let loaded = crate::manifest::read_with_options(
        site_root,
        target.scoped_alias.as_str(),
        root.clone(),
        ctx.mode,
        sources,
        &declaration.options,
    )?;
    let manifest = loaded.manifest;
    let mut module = LoadedModule {
        name: target.scoped_alias.clone(),
        root: loaded.root,
        manifest: loaded.path,
        owner,
        source: Some(declaration.source.clone()),
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
    };

    ancestors.push((canonical, target.scoped_alias.clone()));
    let mut child_lock_buf = BTreeMap::new();
    let child_lock_map = match parent_lock.get_mut(target.lock_key.as_str()) {
        Some(entry) => entry.children_mut(),
        None => &mut child_lock_buf,
    };
    let mut active_locked = BTreeSet::new();
    let declared_children = module.modules.clone();
    for (sub_alias_str, child_decl) in declared_children {
        let enabled =
            crate::options::enabled_by(true, child_decl.enabled_by.as_ref(), &module.options)
                .map_err(anyhow::Error::msg)
                .with_context(|| {
                    format!(
                        "module `{}` child module `{sub_alias_str}`",
                        target.scoped_alias
                    )
                })?;
        if !enabled {
            continue;
        }
        let sub_alias = ModuleAlias::parse(&sub_alias_str)?;
        let child_id = target.scoped_alias.child(&sub_alias);
        module
            .child_aliases
            .insert(sub_alias.clone(), child_id.clone());
        let child_decl = child_decl.into_module_declaration();
        let child_target = crate::inputs::AcquireTarget {
            scoped_alias: &child_id,
            lock_key: &sub_alias,
            owner_root: &module.root,
        };
        let child_owner = ModuleOwner::Module {
            id: module.name.clone(),
            root: module.root.clone(),
        };
        let keep_lock = matches!(
            child_decl.source,
            ModuleSource::Git { .. } | ModuleSource::Tarball { .. }
        );
        acquire_node(
            ctx,
            site_root,
            &child_target,
            child_owner,
            &child_decl,
            child_lock_map,
            ancestors,
            out,
        )?;
        let keep_local_for_nested = matches!(child_decl.source, ModuleSource::Local { .. })
            && child_lock_map
                .get(sub_alias.as_str())
                .is_some_and(|entry| !entry.children().is_empty());
        if keep_lock || keep_local_for_nested {
            active_locked.insert(sub_alias.into_string());
        }
    }
    child_lock_map.retain(|key, _| active_locked.contains(key));
    ancestors.pop();

    out.insert(
        target.scoped_alias.as_str().to_owned(),
        AcquiredModule {
            declaration: declaration.clone(),
            module,
        },
    );
    Ok(())
}

/// Derive one contribution from the declarations and the acquired module roots.
///
/// Pure: no lock write, no cache, no configuration mutation.
pub fn prepare(ctx: &Context<'_>, acquired: &Acquired) -> Result<Contribution> {
    let mut pkgs = Vec::new();
    let mut dep_owners: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    let mut mods = BTreeMap::new();
    let mut assets = Vec::new();
    for (alias, item) in &acquired.modules {
        let pkg = &item.module;
        if pkg.owner.is_site() {
            // Query-only modules have no public library. Their entry is compiled by FileId.
            let entry = pkg
                .typst
                .as_ref()
                .and_then(|typst| typst.entry.as_deref())
                .unwrap_or_else(|| Path::new("__tola_no_public_entry.typ"));
            pkgs.push(LocalPackage::new(
                package_spec(alias)?,
                &pkg.root,
                virtual_path(entry)?,
            )?);
        } else if let Some(entry) = pkg.typst.as_ref().and_then(|typst| typst.entry.as_deref()) {
            let local_alias = pkg.name.leaf();
            if let Some((prev_parent, prev_root)) = dep_owners.get(local_alias) {
                ensure!(
                    prev_root == &pkg.root,
                    "private child Typst libraries collide on `@tola-deps/{local_alias}:0.0.0` (from `{prev_parent}` and `{alias}`)"
                );
            } else {
                dep_owners.insert(local_alias.to_owned(), (alias.clone(), pkg.root.clone()));
                pkgs.push(LocalPackage::new(
                    dep_package_spec(local_alias)?,
                    &pkg.root,
                    virtual_path(entry)?,
                )?);
            }
        }
        if let Some(asset) =
            module_asset_declarations(alias, pkg, item.declaration.assets.as_ref())?
        {
            assets.push(asset);
        }
        mods.insert(alias.clone(), pkg.clone());
    }
    let packages = Arc::new(LocalPackages::new(pkgs, ["tola-modules", "tola-deps"])?);
    Ok(Contribution {
        prepared: ModuleBundle {
            modules: mods,
            packages,
            assets,
        },
        watch: watch_inputs(ctx.site_root, acquired)?,
    })
}

fn watch_inputs(project_root: &Path, acquired: &Acquired) -> Result<HookWatchInputs> {
    let mut inputs = HookWatchInputs::default();
    for item in acquired.modules.values() {
        crate::hooks::watch_inputs(project_root, &item.module, &mut inputs)?;
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
        alias: module.name.as_str().to_owned(),
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

fn module_asset_declarations(
    alias: &str,
    module: &LoadedModule,
    policy: Option<&crate::declaration::ModuleAssetRules>,
) -> Result<Option<AssetContribution>> {
    let default = crate::declaration::ModuleAssetRules::default();
    let policy = policy.unwrap_or(&default);
    if !policy.enable {
        return Ok(None);
    }
    let Some(decl) = &module.assets else {
        ensure!(
            policy.replace.is_empty(),
            "module `{alias}` declares no `assets`"
        );
        return Ok(None);
    };
    for (name, src) in &policy.replace {
        tola_address::OutputPath::parse(name).with_context(|| {
            format!("`modules.{alias}.assets.replace` has an invalid asset name `{name}`")
        })?;
        ensure!(
            crate::declaration::is_relative_path(src),
            "`modules.{alias}.assets.replace` must name a path inside the site"
        );
    }
    Ok(Some(AssetContribution {
        alias: alias.to_owned(),
        root: module.root.clone(),
        declarations: decl.clone(),
        options: module.options.clone(),
        replace: policy.replace.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ModuleAlias;
    use std::fs;
    use tola_input::InputScope;

    #[test]
    fn recursive_acquire_child_and_enabled_by() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = dir.path();
        let docs_dir = site_root.join("theme");
        let child_dir = docs_dir.join("tailwind");
        fs::create_dir_all(&child_dir).unwrap();
        fs::write(
            child_dir.join("tola-module.toml"),
            "[options]\ninput = 'styles/tailwind.css'\nminify = false\n",
        )
        .unwrap();
        fs::write(
            docs_dir.join("tola-module.toml"),
            r#"
[options]
styles = true

[modules.tailwind]
path = "tailwind"
enabled-by = "styles"

[modules.tailwind.options]
input = "styles/site.css"
minify = true
"#,
        )
        .unwrap();

        let decls = BTreeMap::from([(
            "docs".to_owned(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: PathBuf::from("theme"),
                },
                ..Default::default()
            },
        )]);
        let ctx = Context {
            site_root,
            mode: InputMode::new(InputScope::Online, false),
            vendor: None,
            cancellation: &BundleCancellation::default(),
            sources: &[],
        };

        let acquired = acquire(&ctx, &decls).unwrap();
        assert!(acquired.modules.contains_key("docs"));
        let child = &acquired.modules["docs/tailwind"].module;
        assert_eq!(
            child.owner,
            ModuleOwner::Module {
                id: ModuleId::parse("docs").unwrap(),
                root: docs_dir.canonicalize().unwrap(),
            }
        );
        assert_eq!(child.options["input"].as_str(), Some("styles/site.css"));
        assert_eq!(child.options["minify"].as_bool(), Some(true));
        assert_eq!(
            child.child_aliases,
            BTreeMap::<ModuleAlias, ModuleId>::new()
        );

        let lock = InputLock::read(site_root).unwrap().unwrap();
        assert_eq!(lock.modules.keys().collect::<Vec<_>>(), ["docs"]);
        assert!(lock.modules["docs"].children().is_empty());

        let disabled = BTreeMap::from([(
            "docs".to_owned(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: PathBuf::from("theme"),
                },
                options: toml::from_str("styles = false").unwrap(),
                ..Default::default()
            },
        )]);
        let acquired_disabled = acquire(&ctx, &disabled).unwrap();
        assert!(!acquired_disabled.modules.contains_key("docs/tailwind"));
    }

    #[test]
    fn child_local_escape_and_cycle_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = dir.path();
        let escape_parent = site_root.join("escape-parent");
        let outside = site_root.join("outside");
        fs::create_dir_all(&escape_parent).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("tola-module.toml"), "").unwrap();
        fs::write(
            escape_parent.join("tola-module.toml"),
            "[modules.bad]\npath = '../outside'\n",
        )
        .unwrap();

        let ctx = Context {
            site_root,
            mode: InputMode::new(InputScope::Online, true),
            vendor: None,
            cancellation: &BundleCancellation::default(),
            sources: &[],
        };
        let escape_decls = BTreeMap::from([(
            "parent".to_owned(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: PathBuf::from("escape-parent"),
                },
                ..Default::default()
            },
        )]);
        assert!(acquire(&ctx, &escape_decls).is_err());

        let child_target = crate::inputs::AcquireTarget {
            scoped_alias: &ModuleId::parse("parent/bad").unwrap(),
            lock_key: &ModuleAlias::parse("bad").unwrap(),
            owner_root: &escape_parent,
        };
        let bad_decl = ModuleDeclaration {
            source: ModuleSource::Local {
                path: PathBuf::from("../outside"),
            },
            ..Default::default()
        };
        let err = crate::inputs::acquire_one(
            site_root,
            ctx.mode,
            None,
            &child_target,
            &bad_decl,
            None,
            &BundleCancellation::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            crate::inputs::ModuleRootError::ChildOutsideParent { .. }
        ));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../outside", escape_parent.join("link")).unwrap();
            fs::write(
                escape_parent.join("tola-module.toml"),
                "[modules.bad]\npath = 'link'\n",
            )
            .unwrap();
            assert!(acquire(&ctx, &escape_decls).is_err());
        }

        let a = site_root.join("a");
        fs::create_dir_all(&a).unwrap();
        fs::write(
            a.join("tola-module.toml"),
            "[modules.self-loop]\npath = '.'\n",
        )
        .unwrap();
        let cycle_decls = BTreeMap::from([(
            "a".to_owned(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: PathBuf::from("a"),
                },
                ..Default::default()
            },
        )]);
        let err = acquire(&ctx, &cycle_decls).unwrap_err();
        assert!(
            err.downcast_ref::<crate::inputs::ModuleRootError>()
                .is_some_and(|e| matches!(e, crate::inputs::ModuleRootError::Cycle { .. }))
        );
    }
}
