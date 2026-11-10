//! Build configured assets and reuse outputs whose source reads still match.

#![expect(
    clippy::mutable_key_type,
    reason = "FileSelection compares immutable patterns, never the matcher's internal cache"
)]

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
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
    AssetSource, ObservationKey, ResolvedSource, check_source_boundary, check_tree_overlap,
    observe_source, read_source,
};

/// Immutable configured assets shared across clones and unchanged rebuilds.
#[derive(Clone, Debug)]
pub(crate) struct ConfiguredAssets {
    generation: Arc<ConfiguredAssetGeneration>,
}

impl ConfiguredAssets {
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
struct ConfiguredAssetGeneration {
    minify: MinifiedLanguages,
    cache_busting: bool,
    assets: Vec<BuiltAsset>,
    boundary: tola_typst::SourceBoundary,
}

impl ConfiguredAssetGeneration {
    fn publishes_like(&self, config: &ResolvedSiteConfig) -> bool {
        self.minify
            == MinifiedLanguages::new(config.build.minify.css, config.build.minify.javascript)
            && self.cache_busting == config.assets.cache_busting
    }

    /// Whether this generation published exactly these declarations under the current options.
    fn serves(&self, config: &ResolvedSiteConfig, declarations: &[AssetDeclaration]) -> bool {
        let published = self.assets.iter().map(|asset| &asset.declaration);
        self.publishes_like(config) && published.eq(declarations)
    }

    /// Whether a changed path falls under a source this generation read.
    fn reads_changed_path(&self, changed_paths: &[PathBuf]) -> bool {
        self.assets
            .iter()
            .any(|asset| asset.source_identity.changed_paths_intersect(changed_paths))
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
    fn seed_minification(&self, config: &ResolvedSiteConfig, minification: &mut AssetMinification) {
        for asset in &self.assets {
            for (source, (_, bytes)) in asset.observation.files().iter().zip(asset.outputs.iter()) {
                let source_path = asset
                    .declaration
                    .source
                    .logical_source
                    .join(source.relative_path());
                if asset.minify_warnings.iter().any(|warning| {
                    warning.path() == tola_diagnostic::display_path(&source_path, config.get_root())
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

impl ConfiguredAssets {
    pub(crate) fn entries(&self) -> impl Iterator<Item = &(AssetOutput, Arc<[u8]>)> {
        self.generation
            .assets
            .iter()
            .flat_map(|asset| asset.outputs.iter())
    }

    /// URL values come from this generation's declarations and output bytes.
    pub(crate) fn asset_urls(&self, config: &ResolvedSiteConfig) -> Result<super::AssetUrls> {
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
    fn revalidated(&self, config: &ResolvedSiteConfig) -> Result<Self> {
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
                    InputKind::ConfiguredAssets,
                    true,
                    source.logical_path(),
                    source.physical_path(),
                    source_prefix,
                    selection,
                );
            } else {
                observation.path_pair(
                    InputKind::ConfiguredAssets,
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
        config: &ResolvedSiteConfig,
        assets: crate::modules::prepared::AppliedAssets<'_>,
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
    config: &ResolvedSiteConfig,
    assets: crate::modules::prepared::AppliedAssets<'_>,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &crate::cancellation::BuildCancellation,
    previous: Option<&ConfiguredAssets>,
    changed_paths: &[PathBuf],
) -> Result<ConfiguredAssets> {
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
    let observations = observe_resolved_sources(
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
        config.get_root(),
        cancellation,
    )?;
    Ok(ConfiguredAssets {
        generation: Arc::new(ConfiguredAssetGeneration {
            minify,
            cache_busting: config.assets.cache_busting,
            assets,
            boundary: boundary.clone(),
        }),
    })
}

fn validate_declared_sources(
    config: &ResolvedSiteConfig,
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
    config: &ResolvedSiteConfig,
    declarations: &[AssetDeclaration],
    reusable: &ReusableAssets<'_>,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<ResolvedSource>> {
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
                return Ok(ResolvedSource {
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
fn observe_resolved_sources(
    config: &ResolvedSiteConfig,
    boundary: &tola_typst::SourceBoundary,
    resolved_sources: &[ResolvedSource],
    reusable: &ReusableAssets<'_>,
    config_unchanged: bool,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<BTreeMap<ObservationKey, Arc<FilesystemSourceObservation>>> {
    let mut observation_sources = BTreeMap::<ObservationKey, Vec<&ResolvedSource>>::new();
    for resolved in resolved_sources {
        observation_sources
            .entry(resolved.observation_key())
            .or_default()
            .push(resolved);
    }
    let observe = |(key, sources): (ObservationKey, Vec<&ResolvedSource>)| -> Result<_> {
        cancellation.ensure_active()?;
        let previous_observation = reusable.observations.get(&key).copied();
        let changed_source = sources.iter().copied().find(|source| {
            source
                .source_identity
                .changed_paths_intersect(changed_paths)
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
    config: &ResolvedSiteConfig,
    asset: &AssetDeclaration,
    observation: &FilesystemSourceObservation,
    languages: MinifiedLanguages,
    cache_busting: bool,
    minification: &mut AssetMinification,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<(AssetOutput, Arc<[u8]>)>> {
    let display_root = config.get_root();
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::asset::AssetSourceError;
    use crate::asset::tests::{file_declaration, published_url};
    use crate::config::section::assets::{AssetTreeDeclaration, AssetUrlPrefix};

    fn check_urls(config: &ResolvedSiteConfig) -> super::super::AssetUrls {
        super::super::AssetUrls::for_check_with(
            config,
            crate::modules::prepared::AppliedAssets::project_only(config),
            &crate::cancellation::BuildCancellation::default(),
        )
        .expect("configured declarations resolve before building")
    }

    fn tree_declaration(
        source: impl Into<PathBuf>,
        prefix: impl AsRef<str>,
    ) -> AssetTreeDeclaration {
        AssetTreeDeclaration::new(source, AssetUrlPrefix::parse(prefix.as_ref()).unwrap())
    }

    fn build(
        config: &ResolvedSiteConfig,
        previous: Option<&ConfiguredAssets>,
        changed: &[PathBuf],
    ) -> anyhow::Result<ConfiguredAssets> {
        build_configured_assets(
            config,
            crate::modules::prepared::AppliedAssets::project_only(config),
            &crate::resources::source_boundary(config, tola_input::InputScope::Online),
            &crate::cancellation::BuildCancellation::default(),
            previous,
            changed,
        )
    }

    #[cfg(unix)]
    #[test]
    fn pure_assets_reject_host_aliases() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("site");
        let mut config = crate::config::tests::load_test_config(&root, "");
        let outside = directory.path().join("host.css");
        fs::write(&outside, b"body { color: red; }").unwrap();
        let alias = root.join("theme.css");
        std::os::unix::fs::symlink(&outside, &alias).unwrap();
        config.assets.files = vec![file_declaration(&alias, "/theme.css")];
        config.build.minify.css = false;
        let cancellation = crate::cancellation::BuildCancellation::new();
        let previous = built(&config);
        let mut entries = previous.entries();
        let (_, bytes) = entries.next().expect("one published asset");
        assert_eq!(entries.count(), 0, "one published asset");
        assert_eq!(bytes.as_ref(), b"body { color: red; }");
        let pure = crate::resources::source_boundary(&config, tola_input::InputScope::Pure);
        assert!(
            build_configured_assets(
                &config,
                crate::modules::prepared::AppliedAssets::project_only(&config),
                &pure,
                &cancellation,
                Some(&previous),
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn missing_declared_source_fails_discovery() {
        let directory = TempDir::new().unwrap();

        let mut tree_config = crate::config::tests::load_test_config(directory.path(), "");
        tree_config.assets.trees = vec![tree_declaration(
            directory.path().join("missing-tree"),
            "/assets",
        )];
        let tree_error = build(&tree_config, None, &[]).unwrap_err();
        assert!(matches!(
            tree_error.downcast_ref::<AssetSourceError>(),
            Some(AssetSourceError::SourceMissing { path, key: "assets.trees" })
                if path == "missing-tree"
        ));

        let mut file_config = crate::config::tests::load_test_config(directory.path(), "");
        file_config.assets.files = vec![file_declaration(
            directory.path().join("missing-file"),
            "/favicon.ico",
        )];
        let file_error = build(&file_config, None, &[]).unwrap_err();
        assert!(matches!(
            file_error.downcast_ref::<AssetSourceError>(),
            Some(AssetSourceError::SourceMissing { path, key: "assets.files" })
                if path == "missing-file"
        ));
    }

    /// The site directories a tree may not resolve into, with the diagnostic
    /// fragment each is rejected by.
    fn exclusive_site_roots(directory: &TempDir) -> [(PathBuf, &'static str); 3] {
        let content = directory.path().join("content");
        let output = directory.path().join("public");
        let internal = directory.path().join(tola_input::INTERNAL_DIR);
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&output).unwrap();
        fs::create_dir_all(&internal).unwrap();
        [
            (content, "content directory"),
            (output, "output directory"),
            (internal, "`.tola` directory"),
        ]
    }

    fn assert_tree_root_rejected(directory: &TempDir, source: &Path, expected: &str) {
        fs::create_dir_all(source).unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.trees = vec![tree_declaration(source, "/assets")];
        let error = build(&config, None, &[]).unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
    }

    /// A tree may not live in a directory the build owns.
    #[test]
    fn exclusive_site_roots_are_rejected() {
        let directory = TempDir::new().unwrap();

        for (root, expected) in exclusive_site_roots(&directory) {
            assert_tree_root_rejected(&directory, &root.join("assets"), expected);
        }
    }

    /// A symlinked tree root resolving into an owned directory is rejected the same way.
    #[cfg(unix)]
    #[test]
    fn aliased_site_roots_are_rejected() {
        let directory = TempDir::new().unwrap();

        for (index, (root, expected)) in exclusive_site_roots(&directory).iter().enumerate() {
            let alias = directory.path().join(format!("asset-alias-{index}"));
            std::os::unix::fs::symlink(root, &alias).unwrap();
            assert_tree_root_rejected(&directory, &alias, expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_protected_root_is_rechecked() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        let initial_output = directory.path().join("initial-output");
        let output_link = directory.path().join("public-link");
        fs::create_dir_all(&assets).unwrap();
        fs::create_dir_all(&initial_output).unwrap();
        fs::write(assets.join("asset.bin"), b"asset").unwrap();
        symlink(&initial_output, &output_link).unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = output_link.clone();
        config.assets.trees = vec![tree_declaration(&assets, "/assets")];
        let published = built(&config);

        fs::remove_file(&output_link).unwrap();
        symlink(&assets, &output_link).unwrap();
        let error = build(
            &config,
            Some(&published),
            std::slice::from_ref(&output_link),
        )
        .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<AssetSourceError>(),
            Some(AssetSourceError::OverlapsProtected { path, .. }) if path == "assets"
        ));
    }

    /// Each shape of tree member Tola cannot publish, with the reason its refusal names.
    #[cfg(unix)]
    #[test]
    fn unpublishable_tree_members_are_rejected() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        for (label, reason) in [
            ("live.sock", "not a regular file"),
            ("file-link", "symbolic link"),
            ("directory-link", "symbolic link"),
            ("missing-link", "symbolic link"),
            ("query?name.txt", "cannot be published"),
            ("fragment#name.txt", "cannot be published"),
            ("percent%name.txt", "cannot be published"),
        ] {
            let directory = TempDir::new().unwrap();
            let assets = directory.path().join("assets");
            fs::create_dir_all(&assets).unwrap();
            let _listener =
                (label == "live.sock").then(|| UnixListener::bind(assets.join(label)).unwrap());
            match label.strip_suffix("-link") {
                Some(kind) => {
                    let target = directory.path().join(format!("{kind}-target"));
                    match kind {
                        "file" => fs::write(&target, b"target").unwrap(),
                        "directory" => fs::create_dir(&target).unwrap(),
                        "missing" => {}
                        _ => unreachable!(),
                    }
                    symlink(&target, assets.join(label)).unwrap();
                }
                None if label.ends_with(".txt") => {
                    fs::write(assets.join(label), b"asset").unwrap();
                }
                None => {}
            }
            let mut config = crate::config::tests::load_test_config(directory.path(), "");
            config.assets.trees = vec![tree_declaration(&assets, "/assets")];

            let message = build(&config, None, &[]).unwrap_err().to_string();

            assert!(message.contains(label), "{label}: {message}");
            assert!(message.contains(reason), "{label}: {message}");
            if label.ends_with(".txt") {
                // The read-only check refuses the same member before reading asset bytes.
                let checked = super::super::AssetUrls::for_check_with(
                    &config,
                    crate::modules::prepared::AppliedAssets::project_only(&config),
                    &crate::cancellation::BuildCancellation::default(),
                )
                .unwrap_err()
                .to_string();
                assert!(checked.contains(label), "{checked}");
                assert!(checked.contains(reason), "{checked}");
            }
        }

        // An exact `assets.files` declaration refuses the same shape.
        let directory = TempDir::new().unwrap();
        let socket = directory.path().join("live.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.files = vec![file_declaration(&socket, "/live.sock")];
        let message = build(&config, None, &[]).unwrap_err().to_string();
        assert!(message.contains("not a regular file"), "{message}");
        assert!(message.contains("live.sock"), "{message}");
    }

    #[test]
    fn asset_build_cancellation() {
        let directory = TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        let error = build_configured_assets(
            &config,
            crate::modules::prepared::AppliedAssets::project_only(&config),
            &crate::resources::source_boundary(&config, tola_input::InputScope::Online),
            &cancellation,
            None,
            &[],
        )
        .unwrap_err();

        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<crate::cancellation::BuildCancelled>(),
            Some(crate::cancellation::BuildCancelled)
        )));
    }

    #[test]
    fn unminifiable_source_publishes_unchanged() {
        let directory = TempDir::new().unwrap();
        for (name, bytes) in [
            ("invalid.css", b"@media (".as_slice()),
            ("invalid.js", b"function =".as_slice()),
        ] {
            let source = directory.path().join(name);
            fs::write(&source, bytes).unwrap();
            let mut config = crate::config::tests::load_test_config(directory.path(), "");
            config.assets.files = vec![file_declaration(&source, format!("/{name}"))];
            config.build.minify.css = true;
            config.build.minify.javascript = true;
            let assets = built(&config);

            let warnings = assets.minify_warnings();
            let [warning] = &warnings[..] else {
                panic!("expected one warning for {name}, got {warnings:?}");
            };
            assert!(
                matches!(warning, AssetMinifyWarning::Syntax { path, .. } if path == name),
                "{warning:?}"
            );
            let published = assets
                .entries()
                .find(|(output, _)| {
                    output
                        .logical_source
                        .file_name()
                        .is_some_and(|file| file == std::ffi::OsStr::new(name))
                })
                .map(|(_, bytes)| bytes)
                .expect("the source is published");
            assert_eq!(&**published, bytes, "{name} keeps its own bytes");
        }
    }

    #[test]
    fn one_tree_two_prefixes() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("app.css"), b".a { color: red; }").unwrap();

        let mut config = crate::config::tests::load_test_config(
            directory.path(),
            r#"
[assets]
trees = [
  { source = "assets", url-prefix = "/first" },
  { source = "assets", url-prefix = "/second", include = ["asset?"] },
  { source = "assets", url-prefix = "/excluded", exclude = ["assets/*.css"] },
  { source = "assets", url-prefix = "/empty", include = [] },
]
"#,
        );
        config.build.minify.css = true;

        let assets = built(&config);

        let entries = assets.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0.output.as_str(), "first/app.css");
        assert_eq!(entries[1].0.output.as_str(), "second/app.css");
        assert!(entries[0].1.len() < b".a { color: red; }".len());
        assert_eq!(
            entries[0].1, entries[1].1,
            "one source publishes one result"
        );
        let urls = assets.asset_urls(&config).unwrap();
        let origin = crate::asset::AssetOrigin::TreeMember {
            source: config.get_root().join("assets"),
            member: PathBuf::from("app.css"),
        };
        for declared in ["/first/app.css", "/second/app.css"] {
            assert_eq!(urls.origin(declared), Some(&origin));
        }
    }

    /// A tree publishes under its prefix; an exact declaration owns its own URL and keeps the
    /// tree from publishing that member twice; a path that would collide is refused.
    #[test]
    fn declaration_owns_member_url() {
        let directory = TempDir::new().unwrap();
        let tree = directory.path().join("tree");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("member.bin"), b"tree").unwrap();
        let logo = directory.path().join("logo.svg");
        fs::write(&logo, b"<svg></svg>").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&tree, "/assets")];
        config.assets.files = vec![file_declaration(&logo, "/assets/logo.svg")];
        let assets = built(&config);
        assert_eq!(
            published_outputs(&assets),
            ["assets/logo.svg", "assets/member.bin"]
        );
        let urls = assets.asset_urls(&config).unwrap();
        for declared in ["/assets/logo.svg", "/assets/member.bin"] {
            assert_eq!(published_url(&urls, declared).as_deref(), Some(declared));
        }

        // An exact declaration takes the URL away from the tree member it names.
        let (_directory, mut config) = tree_site("[site]\nbase-path = \"/blog/\"");
        config.assets.files = vec![
            file_declaration(config.get_root().join("assets/app.js"), "/assets/app.js"),
            file_declaration(config.get_root().join("assets/theme.css"), "/js/theme.css"),
        ];
        let assets = built(&config);
        assert_eq!(
            published_outputs(&assets),
            ["assets/app.js", "assets/images/logo.svg", "js/theme.css"]
        );
        let urls = assets.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&urls, "/assets/theme.css"),
            None,
            "the tree no longer publishes the member a declaration took"
        );

        // A tree member and a declaration that would publish one path are refused.
        for (tree_prefix, file_url) in [("/downloads/icons", "/downloads"), ("/assets", "/assets")]
        {
            let mut files = crate::config::tests::load_test_config(directory.path(), "");
            files.assets.trees = vec![tree_declaration(&tree, tree_prefix)];
            files.assets.files = vec![file_declaration(&logo, file_url)];
            let error = build(&files, None, &[]).unwrap_err();
            assert!(
                matches!(
                    error.downcast_ref::<AssetSourceError>(),
                    Some(AssetSourceError::OutputsOverlap { .. })
                ),
                "{tree_prefix} / {file_url}: {error:#}"
            );
        }
    }

    fn tree_site(settings: &str) -> (TempDir, ResolvedSiteConfig) {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(assets.join("images")).unwrap();
        fs::write(assets.join("images/logo.svg"), b"<svg></svg>").unwrap();
        fs::write(assets.join("app.js"), "const answer = 40 + 2;").unwrap();
        fs::write(assets.join("theme.css"), "body { color: red; }").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), settings);
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&assets, "/assets")];
        (directory, config)
    }

    fn built(config: &ResolvedSiteConfig) -> ConfiguredAssets {
        build(config, None, &[]).unwrap()
    }

    fn published_outputs(assets: &ConfiguredAssets) -> Vec<String> {
        let mut outputs = assets
            .entries()
            .map(|(output, _)| output.output.as_str().to_owned())
            .collect::<Vec<_>>();
        outputs.sort();
        outputs
    }

    #[test]
    fn tree_members_use_prefix() {
        let (_directory, config) = tree_site("[site]\nbase-path = \"/blog/\"");
        let assets = built(&config);

        assert_eq!(
            published_outputs(&assets),
            [
                "assets/app.js",
                "assets/images/logo.svg",
                "assets/theme.css"
            ]
        );
        let urls = assets.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&urls, "/assets/images/logo.svg").as_deref(),
            Some("/blog/assets/images/logo.svg")
        );
        assert_eq!(
            published_url(&urls, "/assets/theme.css").as_deref(),
            Some("/blog/assets/theme.css")
        );
        assert_eq!(published_url(&urls, "/images/logo.svg").as_deref(), None);

        let checked = check_urls(&config);
        assert_eq!(
            published_url(&checked, "/assets/images/logo.svg").as_deref(),
            Some("/blog/assets/images/logo.svg")
        );
        assert_eq!(
            published_url(&checked, "/assets/theme.css").as_deref(),
            Some("/blog/assets/theme.css")
        );
        assert!(urls.published().eq(checked.published()));
        assert!(urls.is_prepared());
        assert!(!checked.is_prepared());
    }

    #[test]
    fn declaration_edits_keep_addresses() {
        let (directory, mut config) = tree_site("[assets]\ncache-busting = true");
        config.assets.files = vec![file_declaration(
            directory.path().join("assets/theme.css"),
            "/site.css",
        )];
        let first = built(&config);
        let before = first.asset_urls(&config).unwrap();
        let app = published_url(&before, "/assets/app.js")
            .expect("the tree publishes app.js")
            .to_owned();
        assert!(app.contains("?h="), "{app}");

        let added = config.get_root().join("assets/added.svg");
        fs::write(&added, b"<svg></svg>").unwrap();
        let grown = build(&config, Some(&first), std::slice::from_ref(&added)).unwrap();
        let grown_urls = grown.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&grown_urls, "/assets/added.svg")
                .as_deref()
                .map(|url| url.split('?').next().unwrap()),
            Some("/assets/added.svg")
        );
        assert_eq!(
            published_url(&grown_urls, "/assets/app.js").as_deref(),
            Some(app.as_str())
        );
        assert!(before.values_match(&grown_urls));

        config.assets.files.clear();
        let narrowed = built(&config);
        let narrowed_urls = narrowed.asset_urls(&config).unwrap();
        assert_eq!(published_url(&narrowed_urls, "/site.css").as_deref(), None);
        assert_eq!(
            published_url(&narrowed_urls, "/assets/app.js").as_deref(),
            Some(app.as_str())
        );
        assert!(before.values_match(&narrowed_urls));
    }
}
