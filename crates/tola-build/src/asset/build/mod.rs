//! Build configured assets and reuse outputs whose source reads still match.

#![expect(
    clippy::mutable_key_type,
    reason = "FileSelection compares immutable patterns, never the matcher's internal cache"
)]

use anyhow::Result;

use crate::config::SiteConfig;
use crate::filesystem::FilesystemSourceObservation;
use crate::observation::{InputKind, InputObservation, InputPathScope};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tola_input::FilesystemSourceIdentity;

use rayon::prelude::*;
use tola_minify::MinifiedLanguages;

use super::declarations::{self, AssetDeclaration};
use super::minify::{AssetMinification, AssetMinifyKey, AssetMinifyWarning};
use super::output::{AssetOutput, check_paths};
use super::source::{
    AssetSource, LocatedAsset, ObservationKey, check_source_boundary, check_tree_overlap,
    observe_source, read_source,
};

#[cfg(test)]
mod tests;

/// Immutable configured assets shared across clones and unchanged rebuilds.
#[derive(Clone, Debug)]
pub(crate) struct SiteAssets {
    generation: Arc<AssetBuild>,
}

impl SiteAssets {
    /// What minification could not do, one entry per source, in declaration order.
    pub(crate) fn minify_warnings(&self) -> Vec<&AssetMinifyWarning> {
        let mut reported = BTreeSet::new();
        self.generation
            .assets
            .iter()
            .flat_map(|asset| asset.minify_warnings.iter())
            .filter(|warning| reported.insert(warning.path()))
            .collect()
    }
}

#[derive(Debug)]
struct AssetBuild {
    minify: MinifiedLanguages,
    cache_busting: bool,
    assets: Vec<BuiltAsset>,
    boundary: tola_typst::SourceBoundary,
}

impl AssetBuild {
    fn publishes_like(&self, config: &SiteConfig) -> bool {
        self.minify
            == MinifiedLanguages::new(config.build.minify.css, config.build.minify.javascript)
            && self.cache_busting == config.assets.cache_busting
    }

    /// Whether this generation published exactly these declarations under the current options.
    fn serves(&self, config: &SiteConfig, declarations: &[AssetDeclaration]) -> bool {
        let published = self.assets.iter().map(|asset| &asset.declaration);
        self.publishes_like(config) && published.eq(declarations)
    }

    /// Whether a changed path falls under a source this generation read.
    fn reads_changed_path(&self, changed_paths: &[PathBuf]) -> bool {
        self.assets.iter().any(|asset| {
            asset.source_identity.changed_paths_intersect(changed_paths)
                || asset
                    .observation
                    .link_targets_changed(asset.source_identity.logical_path(), changed_paths)
        })
    }

    /// What this generation offers the next build.
    fn reusable(&self) -> ReusableAssets<'_> {
        let mut reusable = ReusableAssets::default();
        for asset in &self.assets {
            reusable.assets.insert(&asset.declaration, asset);
            reusable
                .sources
                .insert(&asset.declaration.source, &asset.source_identity);
            reusable.observations.insert(
                ObservationKey::for_source(&asset.declaration.source, &asset.source_identity),
                &asset.observation,
            );
        }
        reusable
    }

    /// Seed the next build with published bytes so a source is minified once.
    fn seed_minification(&self, config: &SiteConfig, minification: &mut AssetMinification) {
        for asset in &self.assets {
            for (source, (_, bytes)) in asset.observation.files().iter().zip(asset.outputs.iter()) {
                let source_path = asset
                    .declaration
                    .source
                    .logical_source
                    .join(source.relative_path());
                if asset.minify_warnings.iter().any(|warning| {
                    warning.path() == tola_diagnostic::display_path(&source_path, config.root())
                }) {
                    continue;
                }
                minification.seed(
                    AssetMinifyKey::for_source(
                        &source_path,
                        source.digest(),
                        asset.declaration.minified_languages(self.minify),
                    ),
                    Arc::clone(bytes),
                );
            }
        }
    }
}

#[derive(Debug)]
struct BuiltAsset {
    declaration: AssetDeclaration,
    source_identity: FilesystemSourceIdentity,
    observation: Arc<FilesystemSourceObservation>,
    /// One output per published file, in observation order.
    outputs: Arc<[(AssetOutput, Arc<[u8]>)]>,
    /// What this source's minification could not do, kept with the bytes it published.
    minify_warnings: Arc<[AssetMinifyWarning]>,
}

/// Outputs and source reads from the previous build.
#[derive(Default)]
struct ReusableAssets<'a> {
    assets: BTreeMap<&'a AssetDeclaration, &'a BuiltAsset>,
    sources: BTreeMap<&'a AssetSource, &'a FilesystemSourceIdentity>,
    observations: BTreeMap<ObservationKey, &'a Arc<FilesystemSourceObservation>>,
}

impl SiteAssets {
    pub(crate) fn entries(&self) -> impl Iterator<Item = &(AssetOutput, Arc<[u8]>)> {
        self.generation
            .assets
            .iter()
            .flat_map(|asset| asset.outputs.iter())
    }

    /// Configured assets whose effective source paths match one hook's selection.
    pub(crate) fn select(
        &self,
        site: &std::path::Path,
        module: Option<&str>,
        selection: &tola_input::FileSelection,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<serde_json::Value> {
        use super::ModuleAssetInput;

        let mut rows = Vec::new();
        for (asset, _) in self.entries() {
            cancellation.ensure_active()?;
            let source = match (&asset.module, module) {
                (None, None) => asset.logical_source.strip_prefix(site).ok(),
                (Some(asset), None) => match &asset.input {
                    ModuleAssetInput::Project(path) => Some(path.as_path()),
                    ModuleAssetInput::Module(_) => None,
                },
                (Some(asset), Some(alias)) if asset.alias == alias => match &asset.input {
                    ModuleAssetInput::Module(path) => Some(path.as_path()),
                    ModuleAssetInput::Project(_) => None,
                },
                _ => None,
            };
            let Some(source) = source.filter(|path| selection.matches(path)) else {
                continue;
            };
            anyhow::ensure!(
                source.to_str().is_some(),
                "the asset published at `{}` must have a UTF-8 source name",
                asset.output
            );
            rows.push(serde_json::json!({
                "source": tola_diagnostic::render_relative(source),
                "output": asset.output.as_str(),
                "media-type": asset.declaration().media_type().as_str(),
            }));
        }
        Ok(serde_json::Value::Array(rows))
    }

    /// URL values come from this generation's declarations and output bytes.
    pub(crate) fn asset_urls(&self, config: &SiteConfig) -> Result<super::AssetUrls> {
        let published = self.generation.assets.iter().flat_map(|asset| {
            asset
                .observation
                .files()
                .iter()
                .zip(asset.outputs.iter())
                .map(|(source, (output, _))| {
                    asset
                        .declaration
                        .published(config, output, source.relative_path())
                })
        });
        Ok(super::AssetUrls::from_published(published))
    }

    /// Check the retained sources against the current output boundary.
    fn revalidated(&self, config: &SiteConfig) -> Result<Self> {
        for asset in self.generation.assets.iter() {
            check_source_boundary(config, &asset.declaration.source, &asset.source_identity)?;
        }
        Ok(self.clone())
    }

    pub(crate) fn record_watch_inputs(&self, observation: &mut InputObservation) {
        for asset in &self.generation.assets {
            let source = &asset.source_identity;
            if let Some((source_prefix, selection)) = &asset.declaration.source.selection {
                observation.selected_tree_paths(
                    InputKind::SiteAssets,
                    true,
                    source.logical_path(),
                    source.physical_path(),
                    source_prefix,
                    selection,
                );
                for file in asset.observation.linked_files() {
                    observation.path_pair(
                        InputKind::SiteAssets,
                        InputPathScope::Exact,
                        true,
                        &source.logical_path().join(file.relative_path()),
                        file.canonical_target(),
                    );
                }
            } else {
                observation.path_pair(
                    InputKind::SiteAssets,
                    InputPathScope::Exact,
                    true,
                    source.logical_path(),
                    source.physical_path(),
                );
            }
        }
    }

    /// Recheck the exact raw input evidence without rebuilding the assets.
    pub(crate) fn is_fresh_for(
        &self,
        config: &SiteConfig,
        assets: crate::modules::site::AppliedAssets<'_>,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<bool> {
        cancellation.ensure_active()?;
        let observed = (|| -> Result<bool> {
            if !self
                .generation
                .serves(config, &declarations::collect(config, assets)?)
            {
                return Ok(false);
            }
            let mut verified = BTreeSet::new();
            for asset in self.generation.assets.iter() {
                cancellation.ensure_active()?;
                let resolved = read_source(config, &asset.declaration.source, cancellation)?;
                if asset.source_identity != resolved.source_identity {
                    return Ok(false);
                }
                if verified.insert(resolved.observation_key()) {
                    // An empty change list still requires complete membership and byte observation.
                    let current = observe_source(
                        config,
                        &self.generation.boundary,
                        &resolved.id,
                        &resolved.source_identity,
                        cancellation,
                        Some(&asset.observation),
                        &[],
                    )?;
                    if !asset.observation.same_evidence_as(&current) {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        })();
        // Missing or retargeted inputs invalidate the candidate; cancellation stays cancellation.
        cancellation.ensure_active()?;
        Ok(observed.unwrap_or(false))
    }
}

/// Update configured assets without enumerating unaffected sources.
pub(crate) fn build_configured_assets(
    config: &SiteConfig,
    assets: crate::modules::site::AppliedAssets<'_>,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &crate::cancellation::BuildCancellation,
    previous: Option<&SiteAssets>,
    changed_paths: &[PathBuf],
) -> Result<SiteAssets> {
    cancellation.ensure_active()?;
    let declarations = declarations::collect(config, assets)?;
    validate_declared_sources(config, boundary, &declarations)?;
    let previous = previous.filter(|previous| &previous.generation.boundary == boundary);
    let config_unchanged =
        previous.is_some_and(|previous| previous.generation.serves(config, &declarations));
    // A generation exists only after its declarations passed this check.
    if !config_unchanged {
        declarations::check_output_overlap(&declarations)?;
    }
    if let Some(previous) = previous
        && config_unchanged
        && !previous.generation.reads_changed_path(changed_paths)
    {
        cancellation.ensure_active()?;
        return previous.revalidated(config);
    }

    let mut minification = AssetMinification::default();
    let reusable = match previous {
        Some(previous) => {
            previous
                .generation
                .seed_minification(config, &mut minification);
            previous.generation.reusable()
        }
        None => ReusableAssets::default(),
    };
    let resolved_sources = resolve_declared_sources(
        config,
        &declarations,
        &reusable,
        changed_paths,
        cancellation,
    )?;
    check_tree_overlap(config, &resolved_sources)?;
    let resolved_by_id = resolved_sources
        .iter()
        .map(|resolved| (&resolved.id, resolved))
        .collect::<BTreeMap<_, _>>();
    let observations = observe_located_assets(
        config,
        boundary,
        &resolved_sources,
        &reusable,
        config_unchanged,
        changed_paths,
        cancellation,
    )?;

    let minify = MinifiedLanguages::new(config.build.minify.css, config.build.minify.javascript);
    let output_options_match =
        previous.is_some_and(|previous| previous.generation.publishes_like(config));
    let mut assets = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        cancellation.ensure_active()?;
        let source = *resolved_by_id
            .get(&declaration.source)
            .expect("every declaration has a resolved source");
        let observation = Arc::clone(
            observations
                .get(&source.observation_key())
                .expect("every resolved source has one observation"),
        );
        let previous_asset = reusable.assets.get(&declaration).copied();
        let (outputs, minify_warnings) = if let Some(previous_asset) = previous_asset
            && output_options_match
            && previous_asset.source_identity == source.source_identity
            && Arc::ptr_eq(&previous_asset.observation, &observation)
        {
            (
                Arc::clone(&previous_asset.outputs),
                Arc::clone(&previous_asset.minify_warnings),
            )
        } else {
            let outputs = build_outputs(
                config,
                &declaration,
                &observation,
                declaration.minified_languages(minify),
                config.assets.cache_busting,
                &mut minification,
                cancellation,
            )?;
            (outputs.into(), minification.take_warnings().into())
        };
        assets.push(BuiltAsset {
            declaration,
            source_identity: source.source_identity.clone(),
            observation,
            outputs,
            minify_warnings,
        });
    }
    check_paths(
        assets
            .iter()
            .flat_map(|asset| asset.outputs.iter().map(|(output, _)| output)),
        config.root(),
        cancellation,
    )?;
    Ok(SiteAssets {
        generation: Arc::new(AssetBuild {
            minify,
            cache_busting: config.assets.cache_busting,
            assets,
            boundary: boundary.clone(),
        }),
    })
}

fn validate_declared_sources(
    config: &SiteConfig,
    boundary: &tola_typst::SourceBoundary,
    declarations: &[AssetDeclaration],
) -> Result<()> {
    for asset in declarations {
        let identity = FilesystemSourceIdentity::from_path(&asset.source.logical_source);
        check_source_boundary(config, &asset.source, &identity)?;
        boundary.check(&asset.source.logical_source)?;
    }
    Ok(())
}

/// Resolve every declared source, keeping the identity an unchanged source already resolved to.
fn resolve_declared_sources(
    config: &SiteConfig,
    declarations: &[AssetDeclaration],
    reusable: &ReusableAssets<'_>,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<LocatedAsset>> {
    let mut source_ids = declarations
        .iter()
        .map(|declaration| &declaration.source)
        .collect::<Vec<_>>();
    // Declaration ordering groups each logical source before its output fanout.
    source_ids.dedup();
    let resolved = source_ids
        .par_iter()
        .map(|id| {
            cancellation.ensure_active()?;
            if let Some(&source_identity) = reusable.sources.get(id)
                && !source_identity.changed_paths_intersect(changed_paths)
            {
                check_source_boundary(config, id, source_identity)?;
                return Ok(LocatedAsset {
                    id: (*id).clone(),
                    source_identity: source_identity.clone(),
                });
            }
            read_source(config, id, cancellation)
        })
        .collect::<Vec<_>>();
    resolved.into_iter().collect()
}

/// Observe each distinct source once; declarations reaching one canonical source share the result.
fn observe_located_assets(
    config: &SiteConfig,
    boundary: &tola_typst::SourceBoundary,
    resolved_sources: &[LocatedAsset],
    reusable: &ReusableAssets<'_>,
    config_unchanged: bool,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<BTreeMap<ObservationKey, Arc<FilesystemSourceObservation>>> {
    let mut observation_sources = BTreeMap::<ObservationKey, Vec<&LocatedAsset>>::new();
    for resolved in resolved_sources {
        observation_sources
            .entry(resolved.observation_key())
            .or_default()
            .push(resolved);
    }
    let observe = |(key, sources): (ObservationKey, Vec<&LocatedAsset>)| -> Result<_> {
        cancellation.ensure_active()?;
        let previous_observation = reusable.observations.get(&key).copied();
        let changed_source = sources.iter().copied().find(|source| {
            source
                .source_identity
                .changed_paths_intersect(changed_paths)
                || previous_observation.is_some_and(|previous| {
                    previous
                        .link_targets_changed(source.source_identity.logical_path(), changed_paths)
                })
        });
        let observation = match (previous_observation, changed_source) {
            (Some(previous), None) if config_unchanged => Arc::clone(previous),
            _ => {
                let representative = changed_source
                    .unwrap_or_else(|| sources.first().copied().expect("observation has a source"));
                Arc::new(observe_source(
                    config,
                    boundary,
                    &representative.id,
                    &representative.source_identity,
                    cancellation,
                    previous_observation.map(AsRef::as_ref),
                    changed_paths,
                )?)
            }
        };
        Ok((key, observation))
    };
    let observation_results = observation_sources
        .into_iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(observe)
        .collect::<Vec<_>>();
    observation_results.into_iter().collect()
}

fn build_outputs(
    config: &SiteConfig,
    asset: &AssetDeclaration,
    observation: &FilesystemSourceObservation,
    languages: MinifiedLanguages,
    cache_busting: bool,
    minification: &mut AssetMinification,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<(AssetOutput, Arc<[u8]>)>> {
    let display_root = config.root();
    let mut outputs = Vec::with_capacity(observation.files().len());
    for source in observation.files() {
        cancellation.ensure_active()?;
        let source_path = asset.source.logical_source.join(source.relative_path());
        let display_source = tola_diagnostic::display_path(&source_path, display_root);
        let bytes = minification.bytes(
            &source_path,
            &display_source,
            source.digest(),
            source.bytes(),
            languages,
        );
        // The identity names published bytes, not raw input a minifier may rewrite.
        let identity =
            cache_busting.then(|| Arc::<str>::from(tola_typst::ContentDigest::of(&bytes).to_hex()));
        let output = asset.output(source.relative_path(), &source_path, display_root, identity)?;
        outputs.push((output, bytes));
    }
    Ok(outputs)
}
