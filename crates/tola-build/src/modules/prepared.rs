//! Build-side applied site view: the project configuration with one contribution applied.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tola_typst::{BundleCancellation, LocalPackages};

use crate::config::ResolvedSiteConfig;
use tola_module::prepared::{Context as PrepareContext, Contribution, EntrySelection, ModuleEntry};

/// The project's configuration with one contribution applied.
#[derive(Debug, Clone)]
pub struct PreparedSite {
    config: Arc<ResolvedSiteConfig>,
    contribution: Contribution,
    scope: tola_input::InputScope,
}

impl PreparedSite {
    /// Bind the selected entry to the acquired module's Typst package.
    pub fn apply(
        project: Arc<ResolvedSiteConfig>,
        contribution: Contribution,
        scope: tola_input::InputScope,
    ) -> Result<Self> {
        let mut config = project;
        if let Some(alias) = config.module_entry() {
            let module = contribution
                .prepared
                .modules
                .get(alias)
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
        let mut directories = Vec::new();
        for selection in &config.fonts().modules {
            use crate::config::section::typst::fonts::ModuleFonts;

            let (alias, rules) = match selection {
                ModuleFonts::Alias(alias) => (alias, None),
                ModuleFonts::Selection { module, rename } => (module, Some(rename)),
            };
            let module = contribution.prepared.modules.get(alias).with_context(|| {
                format!("`typst.fonts.modules` names undeclared module `{alias}`")
            })?;
            let fonts = module
                .typst
                .as_ref()
                .map(|typst| typst.fonts.as_slice())
                .unwrap_or_default();
            anyhow::ensure!(
                !fonts.is_empty(),
                "`typst.fonts.modules` selects module `{alias}`, which declares no `typst.fonts`"
            );
            let mut rename = std::collections::BTreeMap::new();
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
                    config.get_root(),
                    alias,
                    &module.root,
                    directory,
                    true,
                )
                .with_context(|| {
                    format!("`tola-module.toml` `typst.fonts[{index}]` in module `{alias}`")
                })?;
                directories.push(crate::config::ModuleFontDirectory {
                    alias: alias.clone(),
                    root: module.root.clone(),
                    path: module.root.join(directory),
                    rename: rename.clone(),
                });
            }
        }
        let fresh_archive_roots = if scope == tola_input::InputScope::Pure {
            contribution
                .prepared
                .modules
                .iter()
                .filter(|(alias, _)| {
                    matches!(
                        &config.modules()[*alias].source,
                        tola_module::ModuleSource::Tarball { tarball }
                            if matches!(tarball.location, tola_input::TarballLocation::File { .. })
                    )
                })
                .map(|(_, module)| module.root.clone())
                .collect()
        } else {
            Vec::new()
        };
        let applied = Arc::make_mut(&mut config);
        applied.module_font_directories = directories;
        applied.fresh_archive_roots = fresh_archive_roots;
        Ok(Self {
            config,
            contribution,
            scope,
        })
    }

    pub fn config(&self) -> &ResolvedSiteConfig {
        &self.config
    }

    pub fn config_arc(&self) -> &Arc<ResolvedSiteConfig> {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut ResolvedSiteConfig {
        Arc::make_mut(&mut self.config)
    }

    pub fn contribution(&self) -> &Contribution {
        &self.contribution
    }

    pub fn prepared(&self) -> &tola_module::prepared::PreparedModules {
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
                self.get_root(),
                &module.name,
                &module.root,
                Path::new(id.vpath().get_without_slash()),
                false,
            )?;
        }
        Ok(())
    }

    pub fn loaded_modules(&self) -> impl Iterator<Item = (&str, &tola_module::LoadedModule)> {
        self.prepared()
            .modules
            .iter()
            .map(|(alias, module)| (alias.as_str(), module))
    }

    pub fn local_packages(&self) -> &Arc<LocalPackages> {
        &self.prepared().packages
    }
}

impl std::ops::Deref for PreparedSite {
    type Target = ResolvedSiteConfig;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

fn apply_entry(config: &mut ResolvedSiteConfig, entry: &EntrySelection) -> Result<()> {
    match entry {
        EntrySelection::Module(entry) => {
            config.entry_file =
                Some(tola_module::prepared::package_path(&entry.alias, &entry.entry)?.intern());
            config.build.entry = entry.root.join(&entry.entry);
        }
        EntrySelection::ProjectFile { path } => {
            config.entry_file = Some(
                tola_typst::file_id_from_path(path, config.get_root())
                    .context("the query file must be inside the site")?,
            );
        }
    }
    Ok(())
}

/// Resolve the declared modules and apply their contribution to a project configuration.
pub fn prepare_site(
    config: &ResolvedSiteConfig,
    mode: tola_input::InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<PreparedSite> {
    prepare_site_with_bundle(config, mode, &cancellation.bundle_cancellation())
}

/// Resolve the declared modules under the cancellation the build shares with its host.
pub fn prepare_site_with_bundle(
    config: &ResolvedSiteConfig,
    mode: tola_input::InputMode,
    cancellation: &BundleCancellation,
) -> Result<PreparedSite> {
    let ctx = PrepareContext {
        site_root: config.get_root(),
        mode,
        vendor: config.vendor().path.as_deref(),
        cancellation,
    };
    let acquired = tola_module::prepared::acquire(&ctx, config.modules())?;
    let contribution = tola_module::prepared::prepare(&ctx, &acquired)?;
    PreparedSite::apply(Arc::new(config.clone()), contribution, mode.scope())
}

/// Module assets remain separate from the project's declarations.
#[derive(Clone, Copy)]
pub(crate) struct AppliedAssets<'a> {
    pub(crate) modules: &'a [tola_module::prepared::AssetContribution],
}

impl AppliedAssets<'_> {
    pub(crate) fn project_only(_: &ResolvedSiteConfig) -> Self {
        Self { modules: &[] }
    }
}
