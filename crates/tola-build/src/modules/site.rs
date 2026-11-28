//! Build-side applied site view: the project configuration with one contribution applied.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tola_typst::{BundleCancellation, LocalPackages};

use crate::config::SiteConfig;
use tola_module::bundle::{Context as PrepareContext, Contribution, EntrySelection, ModuleEntry};

/// The project's configuration with one contribution applied.
#[derive(Debug, Clone)]
pub struct LoadedSite {
    config: Arc<SiteConfig>,
    contribution: Contribution,
    scope: tola_input::InputScope,
}
pub(super) fn select_module_fonts<'a>(
    config: &mut SiteConfig,
    scope: tola_input::InputScope,
    modules: impl Iterator<Item = (&'a str, &'a Path, &'a [PathBuf])>,
) -> Result<()> {
    let modules = modules
        .map(|(alias, root, fonts)| (alias, (root, fonts)))
        .collect::<BTreeMap<_, _>>();
    let mut directories = Vec::new();
    for selection in &config.fonts().modules {
        use crate::config::section::typst::fonts::ModuleFonts;

        let (alias, rules) = match selection {
            ModuleFonts::Alias(alias) => (alias, None),
            ModuleFonts::Selection { module, rename } => (module, Some(rename)),
        };
        let &(root, fonts) = modules
            .get(alias.as_str())
            .with_context(|| format!("`typst.fonts.modules` names undeclared module `{alias}`"))?;
        anyhow::ensure!(
            !fonts.is_empty(),
            "`typst.fonts.modules` selects module `{alias}`, which declares no `typst.fonts`"
        );
        let mut rename = BTreeMap::new();
        for (family, target) in rules.into_iter().flatten() {
            anyhow::ensure!(
                !family.trim().is_empty() && !target.trim().is_empty(),
                "`typst.fonts.modules` font rename for `{alias}` needs two nonempty family names"
            );
            anyhow::ensure!(
                rename
                    .insert(family.to_lowercase(), target.clone())
                    .is_none(),
                "`typst.fonts.modules` renames font family `{family}` more than once for `{alias}`"
            );
        }
        let rename = Arc::new(rename);
        for (index, directory) in fonts.iter().enumerate() {
            tola_module::module_set::validate_module_path(
                config.root(),
                alias,
                root,
                directory,
                true,
            )
            .with_context(|| {
                format!("`tola-module.toml` `typst.fonts[{index}]` in module `{alias}`")
            })?;
            directories.push(crate::config::ModuleFontDirectory {
                alias: alias.clone(),
                root: root.to_path_buf(),
                path: root.join(directory),
                rename: rename.clone(),
            });
        }
    }
    let fresh_archive_roots = if scope == tola_input::InputScope::Pure {
        modules
            .iter()
            .filter(|&(alias, _)| {
                matches!(
                    &config.modules()[*alias].source,
                    tola_module::ModuleSource::Tarball { tarball }
                        if matches!(tarball.location, tola_input::TarballLocation::File { .. })
                )
            })
            .map(|(_, (root, _))| root.to_path_buf())
            .collect()
    } else {
        Vec::new()
    };
    config.module_font_directories = directories;
    config.fresh_archive_roots = fresh_archive_roots;
    Ok(())
}

impl LoadedSite {
    /// Bind the selected entry to the acquired module's Typst package.
    pub fn apply(
        project: Arc<SiteConfig>,
        contribution: Contribution,
        scope: tola_input::InputScope,
    ) -> Result<Self> {
        let mut config = project;
        if let Some(alias) = config.module_entry() {
            let module = contribution
                .prepared
                .modules
                .get(alias)
                .filter(|module| module.owner.is_site())
                .with_context(|| format!("the entry module `{alias}` was not acquired"))?;
            let build = module
                .build
                .as_ref()
                .with_context(|| format!("the entry module `{alias}` declares no `build.entry`"))?;
            let entry = EntrySelection::Module(ModuleEntry {
                alias: alias.to_owned(),
                root: module.root.clone(),
                entry: build.entry.clone(),
            });
            apply_entry(Arc::make_mut(&mut config), &entry)?;
        }
        select_module_fonts(
            Arc::make_mut(&mut config),
            scope,
            contribution
                .prepared
                .modules
                .iter()
                .filter(|(_, module)| module.owner.is_site())
                .map(|(alias, module)| {
                    let fonts = module
                        .typst
                        .as_ref()
                        .map(|typst| typst.fonts.as_slice())
                        .unwrap_or_default();
                    (alias.as_str(), module.root.as_path(), fonts)
                }),
        )?;
        apply_module_icons(Arc::make_mut(&mut config), &contribution.prepared.modules)?;
        Ok(Self {
            config,
            contribution,
            scope,
        })
    }

    pub fn config(&self) -> &SiteConfig {
        &self.config
    }

    pub fn config_arc(&self) -> &Arc<SiteConfig> {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut SiteConfig {
        Arc::make_mut(&mut self.config)
    }

    pub fn contribution(&self) -> &Contribution {
        &self.contribution
    }

    pub fn prepared(&self) -> &tola_module::bundle::ModuleBundle {
        &self.contribution.prepared
    }

    pub(crate) fn applied_assets(&self) -> AppliedAssets<'_> {
        AppliedAssets {
            modules: &self.prepared().assets,
        }
    }

    pub fn scope(&self) -> tola_input::InputScope {
        self.scope
    }

    /// Override the entry after preparation, as a query does with one selected entry.
    pub fn select_entry(&mut self, entry: EntrySelection) -> Result<()> {
        apply_entry(Arc::make_mut(&mut self.config), &entry)
    }

    /// Check the selected module entry after before-build generators have run.
    pub fn validate_module_entry(&self) -> Result<()> {
        if let Some(id) = self.entry_file()
            && let typst::syntax::VirtualRoot::Package(package) = id.root()
            && let Some(module) = self.prepared().modules.get(package.name.as_str())
        {
            tola_module::module_set::validate_module_path(
                self.root(),
                module.name.as_str(),
                &module.root,
                Path::new(id.vpath().get_without_slash()),
                false,
            )?;
        }
        Ok(())
    }

    /// Site-declared modules in alias order, excluding private child dependencies.
    pub fn loaded_modules(&self) -> impl Iterator<Item = (&str, &tola_module::LoadedModule)> {
        self.prepared()
            .modules
            .iter()
            .filter(|(_, module)| module.owner.is_site())
            .map(|(alias, module)| (alias.as_str(), module))
    }

    pub fn local_packages(&self) -> &Arc<LocalPackages> {
        &self.prepared().packages
    }
}

/// Mount the acquired modules' icon collections, applying site-written replacements.
pub fn apply_module_icons(
    config: &mut SiteConfig,
    modules: &BTreeMap<String, tola_module::module_set::LoadedModule>,
) -> Result<()> {
    use crate::config::section::IconsConfig;

    config
        .icons
        .collections
        .retain(|name, _| !name.starts_with("modules/"));
    for (alias, module) in modules {
        if !module.owner.is_site() {
            continue;
        }
        let replacements = config.modules()[alias].icons.clone();
        let replacements = replacements.as_ref();
        for name in replacements
            .into_iter()
            .flat_map(|icons| icons.collections.keys())
        {
            anyhow::ensure!(
                module.icons.collections.contains_key(name),
                "`modules.{alias}.icons.collections.{name}` replaces a collection absent from the module manifest"
            );
        }
        for (name, declared) in &module.icons.collections {
            let replacement = replacements.and_then(|icons| icons.collections.get(name));
            let root = if replacement.is_some() {
                config.root()
            } else {
                module.root.as_path()
            };
            let source = replacement.unwrap_or(declared).clone();
            let mut icons = IconsConfig {
                collections: [(name.clone(), source)].into(),
            };
            let mut diagnostics = crate::config::ConfigDiagnostics::new();
            icons.validate(&mut diagnostics);
            icons.validate_paths(&mut diagnostics);
            icons.validate_internal_boundaries(root, &mut diagnostics);
            diagnostics
                .into_result()
                .with_context(|| format!("icon collection `{name}` of module `{alias}`"))?;
            if replacement.is_none() {
                for path in icons.local_paths() {
                    anyhow::ensure!(
                        tola_input::normalize_existing_prefix(&root.join(path)).starts_with(root),
                        "`icons.collections.{name}.path` leaves module `{alias}`"
                    );
                }
            }
            icons.normalize(root);
            let key = tola_icons::IconCollectionKey::new(Some(alias), name)?;
            config.icons.collections.insert(
                key.to_string(),
                icons
                    .collections
                    .remove(name)
                    .expect("validated collection"),
            );
        }
    }
    Ok(())
}

impl std::ops::Deref for LoadedSite {
    type Target = SiteConfig;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

fn apply_entry(config: &mut SiteConfig, entry: &EntrySelection) -> Result<()> {
    match entry {
        EntrySelection::Module(entry) => {
            config.entry_file =
                Some(tola_module::bundle::package_path(&entry.alias, &entry.entry)?.intern());
            config.build.entry = entry.root.join(&entry.entry);
        }
        EntrySelection::ProjectFile { path } => {
            config.entry_file = Some(
                tola_typst::file_id_from_path(path, config.root())
                    .context("the query file must be inside the site")?,
            );
        }
    }
    Ok(())
}

/// Resolve the declared modules and apply their contribution to a project configuration.
pub fn prepare_site(
    config: &SiteConfig,
    mode: tola_input::InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<LoadedSite> {
    prepare_site_with_bundle(config, mode, &cancellation.bundle_cancellation(), &[])
}

/// Resolve the declared modules under the cancellation the build shares with its host.
pub fn prepare_site_with_bundle(
    config: &SiteConfig,
    mode: tola_input::InputMode,
    cancellation: &BundleCancellation,
    sources: &[(PathBuf, Arc<str>)],
) -> Result<LoadedSite> {
    let ctx = PrepareContext {
        site_root: config.root(),
        mode,
        vendor: config.vendor().path.as_deref(),
        cancellation,
        sources,
    };
    let acquired = tola_module::bundle::acquire(&ctx, config.modules())?;
    let contribution = tola_module::bundle::prepare(&ctx, &acquired)?;
    LoadedSite::apply(Arc::new(config.clone()), contribution, mode.scope())
}

/// Module assets remain separate from the project's declarations.
#[derive(Clone, Copy)]
pub(crate) struct AppliedAssets<'a> {
    pub(crate) modules: &'a [tola_module::bundle::AssetContribution],
}

impl AppliedAssets<'_> {
    pub(crate) fn project_only(_: &SiteConfig) -> Self {
        Self { modules: &[] }
    }
}
