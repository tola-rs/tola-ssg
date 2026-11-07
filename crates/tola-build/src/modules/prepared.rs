//! Module identities and project-owned configuration for one build attempt.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use tola_typst::{LocalPackage, LocalPackages};
use typst::foundations::{Dict, IntoValue, Value};
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot, package::PackageSpec};

use super::{ConfiguredModule, configure_module, resolve_modules_with_roots};
use crate::InputScope;
use crate::config::ResolvedSiteConfig;
use crate::config::section::assets::{
    AssetFileDeclaration, AssetSelection, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
};
use crate::inputs::InputMode;

#[derive(Debug, Clone, Default)]
pub(crate) struct ModuleInputs {
    pub scope: InputScope,
    pub packages: Arc<LocalPackages>,
    pub configured: BTreeMap<String, ConfiguredModule>,
    pub validated_options: Option<Arc<super::options::ValidatedOptions>>,
    pub project_hooks: Option<crate::config::section::build::hooks::HooksConfig>,
    asset_count: usize,
    asset_file_count: usize,
    asset_selections: BTreeMap<(std::path::PathBuf, String), AssetSelection>,
}

pub(crate) fn package_spec(alias: &str) -> Result<PackageSpec> {
    format!("@tola-modules/{alias}:0.0.0")
        .parse()
        .map_err(|error| anyhow::anyhow!("module `{alias}` is not a usable package name: {error}"))
}

pub(crate) fn package_path(alias: &str, path: &Path) -> Result<RootedPath> {
    Ok(RootedPath::new(
        VirtualRoot::Package(package_spec(alias)?),
        virtual_path(path)?,
    ))
}

fn virtual_path(path: &Path) -> Result<VirtualPath> {
    VirtualPath::new(path.to_string_lossy().replace('\\', "/")).map_err(|error| {
        anyhow::Error::new(error).context(format!(
            "the module source path `{}` is not usable",
            path.display()
        ))
    })
}

/// Resolve local module manifests, paths, assets, and the selected site entry.
/// This prepares one invocation without executing Typst schemas, hooks, or commands.
pub fn configure(
    base: &ResolvedSiteConfig,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<ResolvedSiteConfig> {
    let mut config = base.clone();
    if let Some(project_hooks) = &config.module_inputs.project_hooks {
        config.build.hooks = project_hooks.clone();
    }
    // A previously prepared driver follows a changed manifest; query entries do not.
    let use_theme_entry = config.entry_origin != crate::config::EntryOrigin::Query;
    let original_assets = config.assets.trees.len() - config.module_inputs.asset_count;
    config.assets.trees.truncate(original_assets);
    let original_files = config.assets.files.len() - config.module_inputs.asset_file_count;
    config.assets.files.truncate(original_files);
    let roots = crate::inputs::module_roots(
        config.get_root(),
        mode,
        config.vendor().path.as_deref(),
        config.modules(),
        cancellation,
    )?;
    let mut configured = BTreeMap::new();
    let mut asset_selections = BTreeMap::new();
    let mut packages = Vec::new();
    for (alias, module) in
        resolve_modules_with_roots(config.get_root(), config.modules(), &roots, mode)?
    {
        // Query-only modules have no public library. Their entry is compiled by FileId.
        let entry = module
            .typst
            .as_ref()
            .map(|entry| entry.entry.as_path())
            .unwrap_or_else(|| Path::new("__tola_no_public_entry.typ"));
        packages.push(LocalPackage::new(
            package_spec(&alias)?,
            &module.root,
            virtual_path(entry)?,
        )?);
        let declaration = &config.modules()[&alias];
        if let Some((tree, files, selection)) = module_asset_declarations(
            config.get_root(),
            &alias,
            &module,
            declaration.assets.as_ref(),
        )? {
            asset_selections.insert(
                (
                    tree.source().to_path_buf(),
                    tree.url_prefix().as_str().to_owned(),
                ),
                selection,
            );
            config.assets.trees.push(tree);
            config.assets.files.extend(files);
        }
        configured.insert(alias, configure_module(&config, module)?);
    }
    if use_theme_entry && let Some(alias) = &config.theme.module {
        let module = &configured
            .get(alias)
            .with_context(|| {
                format!("Tola could not find the configuration of the theme module `{alias}`")
            })?
            .module;
        let site = module
            .site
            .as_ref()
            .with_context(|| format!("the theme module `{alias}` declares no `site.entry`"))?;
        config.entry_file = Some(package_path(alias, &site.entry)?.intern());
        config.entry_origin = crate::config::EntryOrigin::Theme;
        config.build.entry = module.root.join(&site.entry);
    }
    let asset_count = config.assets.trees.len() - original_assets;
    config.module_inputs = Arc::new(ModuleInputs {
        scope: mode.scope(),
        packages: Arc::new(LocalPackages::new(packages, ["tola-modules"])?),
        configured,
        asset_count,
        asset_file_count: config.assets.files.len() - original_files,
        asset_selections,
        validated_options: None,
        project_hooks: None,
    });
    Ok(config)
}

/// The asset declarations one module publishes, with their selection.
fn module_asset_declarations(
    site_root: &Path,
    alias: &str,
    module: &super::ResolvedModule,
    declared: Option<&super::ModuleAssetPolicy>,
) -> Result<
    Option<(
        AssetTreeDeclaration,
        Vec<AssetFileDeclaration>,
        AssetSelection,
    )>,
> {
    let default = super::ModuleAssetPolicy::default();
    let policy = declared.unwrap_or(&default);
    let valid_name = |value: &str| -> Result<()> {
        tola_address::OutputPath::parse(value).map_err(|error| {
            anyhow::anyhow!("the module asset path `{value}` is not usable: {error}")
        })?;
        Ok(())
    };
    for name in policy
        .include
        .iter()
        .flatten()
        .chain(policy.exclude.iter())
        .chain(policy.replace.keys())
    {
        valid_name(name)?;
    }
    for source in policy.replace.values() {
        ensure!(
            super::is_relative_path(source),
            "`modules.{alias}.assets.replace` must name a path inside the site"
        );
    }
    if !policy.enabled {
        return Ok(None);
    }
    let Some(assets) = &module.assets else {
        ensure!(
            declared.is_none_or(|p| p.include.is_none()
                && p.exclude.is_empty()
                && p.replace.is_empty()),
            "module `{alias}` declares no `assets`"
        );
        return Ok(None);
    };
    let mut selection = AssetSelection {
        include: policy.include.clone(),
        exclude: policy.exclude.clone(),
        replaced: policy.replace.keys().cloned().collect(),
    };
    if let Some(include) = &mut selection.include {
        include.sort();
        include.dedup();
    }
    selection.exclude.sort();
    selection.exclude.dedup();
    let module_assets = module.root.join(&assets.directory);
    let canonical_assets = module_assets.canonicalize()?;
    let canonical_site = site_root.canonicalize()?;
    let mut replacements = Vec::new();
    for (target, source) in &policy.replace {
        ensure!(
            selection.selects(target),
            "`{target}` is replaced but not selected by `modules.{alias}.assets`"
        );
        let original = module_assets.join(target).canonicalize().with_context(|| {
            format!("Tola could not read `{target}` in the assets of module `{alias}`")
        })?;
        ensure!(
            original.starts_with(&canonical_assets) && original.is_file(),
            "`{target}` is not a file in the assets of module `{alias}`"
        );
        let source = site_root.join(source);
        let resolved = crate::filesystem::normalize_existing_prefix(&source);
        ensure!(
            resolved.starts_with(&canonical_site),
            "the replacement of `{target}` points outside the site"
        );
        let output = tola_address::OutputPath::parse(&format!("_modules/{alias}/{target}"))?;
        let url = tola_address::asset_url_from_output(&output).to_encoded();
        replacements.push(AssetFileDeclaration::new(source, AssetUrl::parse(&url)?));
    }
    let tree = AssetTreeDeclaration::new(
        module_assets,
        AssetUrlPrefix::parse(&format!("/_modules/{alias}"))?,
    );
    Ok(Some((tree, replacements, selection)))
}

impl ModuleInputs {
    /// The original project declarations precede module-injected replacements.
    pub(crate) fn project_asset_files_len(&self, total: usize) -> usize {
        total - self.asset_file_count
    }

    pub(crate) fn asset_selection(&self, tree: &AssetTreeDeclaration) -> Option<&AssetSelection> {
        self.asset_selections.get(&(
            tree.source().to_path_buf(),
            tree.url_prefix().as_str().to_owned(),
        ))
    }

    /// Recheck the exact manifest bytes and identities that selected this build's modules.
    pub(crate) fn manifests_are_fresh(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<bool> {
        for configured in self.configured.values() {
            let Some(evidence) = &configured.module.manifest_evidence else {
                return Ok(false);
            };
            if !evidence.is_current(cancellation)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn observe_manifests(&self, observation: &mut crate::observation::InputObservation) {
        use crate::observation::{InputKind, InputPathScope};
        for configured in self.configured.values() {
            if let Some(evidence) = &configured.module.manifest_evidence {
                let (root, manifest) = evidence.paths();
                for identity in [root, manifest] {
                    observation.path_pair(
                        InputKind::ModuleManifests,
                        InputPathScope::Exact,
                        true,
                        identity.logical_path(),
                        identity.physical_path(),
                    );
                }
            }
        }
    }

    pub(crate) fn contexts(
        &self,
        config: &ResolvedSiteConfig,
        urls: &crate::asset::AssetUrls,
    ) -> Result<Dict> {
        let mut records = Dict::new();
        let project_root = config.get_root().canonicalize()?;
        for (alias, configured) in &self.configured {
            let module = &configured.module;
            let raw = toml::Value::Table(configured.options.clone().into_iter().collect());
            let options: Value = raw
                .try_into()
                .with_context(|| format!("Tola could not read the options of module `{alias}`"))?;
            let mut slots = Dict::new();
            for (name, default) in &module.slots {
                let path = if config.modules[alias].overrides.contains_key(name) {
                    let physical = &configured.slots[name];
                    let relative = physical.strip_prefix(&project_root)?;
                    RootedPath::new(VirtualRoot::Project, virtual_path(relative)?)
                } else {
                    package_path(alias, default)?
                };
                slots.insert(name.as_str().into(), path.into_value());
            }
            let mut record = Dict::new();
            record.insert("name".into(), alias.as_str().into_value());
            record.insert(
                "package".into(),
                module.package_name.as_deref().unwrap_or(alias).into_value(),
            );
            record.insert(
                "version".into(),
                module
                    .version
                    .map(|version| version.to_string())
                    .into_value(),
            );
            record.insert("options".into(), options);
            record.insert("slots".into(), slots.into_value());
            let policy = config.modules[alias].assets.as_ref();
            let enabled = module.assets.is_some() && policy.is_none_or(|p| p.enabled);
            let mut files = Dict::new();
            let mut asset_urls = Dict::new();
            if enabled {
                let assets_root = module.root.join(
                    &module
                        .assets
                        .as_ref()
                        .expect("enabled asset source")
                        .directory,
                );
                let prefix = format!("/_modules/{alias}/");
                for asset in urls.published() {
                    let Some(relative) = asset.declared.strip_prefix(&prefix) else {
                        continue;
                    };
                    let file = match asset.origin {
                        Some(crate::asset::AssetOrigin::TreeMember { source, member })
                            if source == &assets_root =>
                        {
                            Some(package_path(
                                alias,
                                &module.assets.as_ref().unwrap().directory.join(member),
                            )?)
                        }
                        Some(crate::asset::AssetOrigin::File { source }) => policy
                            .and_then(|p| p.replace.get(relative))
                            .filter(|path| {
                                crate::filesystem::FilesystemSourceIdentity::from_path(
                                    &config.get_root().join(path),
                                )
                                .intersects(
                                    &crate::filesystem::FilesystemSourceIdentity::from_path(source),
                                )
                            })
                            .map(|path| -> Result<RootedPath> {
                                Ok(RootedPath::new(VirtualRoot::Project, virtual_path(path)?))
                            })
                            .transpose()?,
                        _ => None,
                    };
                    if let Some(file) = file {
                        files.insert(relative.into(), file.into_value());
                        asset_urls.insert(relative.into(), asset.address.into_value());
                    }
                }
            }
            let mut asset_info = Dict::new();
            asset_info.insert("enabled".into(), enabled.into_value());
            asset_info.insert("files".into(), files.into_value());
            asset_info.insert("urls".into(), asset_urls.into_value());
            record.insert("assets".into(), asset_info.into_value());
            record.insert(
                "asset-prefix".into(),
                format!("/_modules/{alias}").into_value(),
            );
            records.insert(alias.as_str().into(), record.into_value());
        }
        Ok(records)
    }
}

pub(crate) fn select_query(
    config: &ResolvedSiteConfig,
    module: &super::ResolvedModule,
    query: &super::Query,
) -> Result<ResolvedSiteConfig> {
    ensure!(
        config.modules.contains_key(&module.name),
        "query module is not configured"
    );
    let mut query_config = config.clone();
    query_config.entry_file = Some(package_path(&module.name, &query.entry)?.intern());
    query_config.entry_origin = crate::config::EntryOrigin::Query;
    query_config.build.entry = module.root.join(&query.entry);
    Ok(query_config)
}

pub(crate) fn select_query_file(
    config: &ResolvedSiteConfig,
    entry: &Path,
) -> Result<ResolvedSiteConfig> {
    let mut query_config = config.clone();
    query_config.entry_file = Some(
        tola_typst::file_id_from_path(entry, config.get_root())
            .context("the query file must be inside the site")?,
    );
    query_config.entry_origin = crate::config::EntryOrigin::Query;
    Ok(query_config)
}

#[cfg(test)]
mod tests {}
