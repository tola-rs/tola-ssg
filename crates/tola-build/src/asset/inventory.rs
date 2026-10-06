//! The configured asset inventory: resolving declared sources, observing and rendering
//! their bytes, and deciding whether a retained generation still matches its inputs.

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::filesystem::{
    FilesystemSourceFile, FilesystemSourceIdentity, FilesystemSourceKind,
    FilesystemSourceObservation, FilesystemWatchEvidence, absolute_path,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use tola_minify::MinifiedLanguages;

use super::minify::{AssetMinification, AssetMinifyKey, AssetMinifyWarning};
use super::output::AssetOutput;

/// Immutable configured assets shared across clones and unchanged rebuilds.
///
/// Declarations publishing the same physical source share an observation.
/// Updates replace changed observations without retaining previous generations.
#[derive(Clone, Debug)]
pub(crate) struct ConfiguredAssetInventory {
    generation: Arc<ConfiguredAssetGeneration>,
}

impl ConfiguredAssetInventory {
    /// What minification could not do, one entry per source, in declaration order.
    ///
    /// A source Tola cannot minify publishes its own bytes, so this is a warning rather than a
    /// failure, and a reused inventory reports the same warnings it reported when it was rendered.
    /// One source published at several addresses is one warning.
    pub(crate) fn minify_warnings(&self) -> Vec<&AssetMinifyWarning> {
        let mut warnings = Vec::new();
        for warning in self
            .generation
            .assets
            .iter()
            .flat_map(|asset| asset.minify_warnings.iter())
        {
            if !warnings
                .iter()
                .any(|reported: &&AssetMinifyWarning| reported.path() == warning.path())
            {
                warnings.push(warning);
            }
        }
        warnings
    }
}

#[derive(Debug)]
struct ConfiguredAssetGeneration {
    spec: ConfiguredAssetSpec,
    assets: Arc<[ConfiguredAssetEntry]>,
    boundary: tola_typst::SourceBoundary,
}

#[derive(Clone, Debug)]
struct ConfiguredAssetEntry {
    spec: ConfiguredAsset,
    source_identity: FilesystemSourceIdentity,
    observation: Arc<FilesystemSourceObservation>,
    entries: Arc<[(AssetOutput, Arc<[u8]>)]>,
    /// What this source's minification could not do, kept with the bytes it published.
    minify_warnings: Arc<[AssetMinifyWarning]>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ConfiguredSourceId {
    kind: FilesystemSourceKind,
    logical_source: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ConfiguredAsset {
    source_id: ConfiguredSourceId,
    output: ConfiguredAssetOutput,
    /// The site-root URL this declaration was written with, kept as the stable
    /// key authors reference. A tree publishes under a prefix, so it has none.
    declared_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ConfiguredAssetOutput {
    TreePrefix(crate::config::section::AssetUrlPrefix),
    File { output: tola_address::OutputPath },
}

impl ConfiguredAssetOutput {
    fn portable_key(&self) -> Vec<String> {
        match self {
            Self::TreePrefix(url_prefix) => url_prefix
                .as_str()
                .trim_matches('/')
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(tola_address::portable_collision_key)
                .collect(),
            Self::File { output } => output.portable_key(),
        }
    }

    fn display_coordinate(&self) -> String {
        match self {
            Self::TreePrefix(url_prefix) => url_prefix.to_string(),
            Self::File { output } => format!("/{output}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ConfiguredAssetSpec {
    assets: Vec<ConfiguredAsset>,
    minify: MinifiedLanguages,
    /// Whether published bytes are identified for `asset-url()`.
    cache_busting: bool,
}

impl ConfiguredAssetSpec {
    fn for_config(config: &ResolvedSiteConfig) -> Self {
        let mut assets = config
            .assets
            .trees
            .iter()
            .map(|declaration| ConfiguredAsset {
                source_id: ConfiguredSourceId {
                    kind: FilesystemSourceKind::Tree,
                    logical_source: absolute_path(declaration.source()),
                },
                output: ConfiguredAssetOutput::TreePrefix(declaration.url_prefix().clone()),
                declared_url: None,
            })
            .chain(
                config
                    .assets
                    .files
                    .iter()
                    .map(|declaration| ConfiguredAsset {
                        source_id: ConfiguredSourceId {
                            kind: FilesystemSourceKind::File,
                            logical_source: absolute_path(declaration.source()),
                        },
                        output: ConfiguredAssetOutput::File {
                            output: declaration.url().output_path().clone(),
                        },
                        declared_url: Some(declaration.url().as_str().to_owned()),
                    }),
            )
            .collect::<Vec<_>>();
        assets.sort_unstable();
        Self {
            assets,
            minify: MinifiedLanguages::new(config.build.minify.css, config.build.minify.javascript),
            cache_busting: config.assets.cache_busting,
        }
    }

    /// Whether both specs turn one observation into the same published entries.
    ///
    /// An observation is unchanged bytes, so only the settings that decide an
    /// entry itself matter here: its minified bytes and whether those bytes
    /// hold an identity.
    fn publishes_like(&self, other: &Self) -> bool {
        self.minify == other.minify && self.cache_busting == other.cache_busting
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AssetObservationKey {
    kind: FilesystemSourceKind,
    canonical: PathBuf,
}

impl AssetObservationKey {
    fn for_source(kind: FilesystemSourceKind, source: &FilesystemSourceIdentity) -> Self {
        Self {
            kind,
            canonical: source
                .canonical_path()
                .expect("resolved configured sources have canonical targets")
                .to_path_buf(),
        }
    }
}

#[derive(Clone, Debug)]
struct ResolvedAssetSource {
    id: ConfiguredSourceId,
    source_identity: FilesystemSourceIdentity,
}

impl ResolvedAssetSource {
    fn observation_key(&self) -> AssetObservationKey {
        AssetObservationKey::for_source(self.id.kind, &self.source_identity)
    }
}

impl ConfiguredAssetInventory {
    pub(crate) fn entries(&self) -> impl Iterator<Item = &(AssetOutput, Arc<[u8]>)> {
        self.generation
            .assets
            .iter()
            .flat_map(|asset| asset.entries.iter())
    }

    /// Resolve every URL this site publishes against this inventory's own outputs.
    ///
    /// The dictionary is keyed by the site-root URL an author writes: a
    /// `assets.files` declaration's own `url`, or a `assets.trees`
    /// member's URL prefix joined with its path inside the source tree, such as
    /// `/assets/images/logo.svg`. A value is the browser URL of that exact
    /// output, with the identity of its published bytes appended as
    /// `?h=<identity>` when the site asked `[assets] cache-busting` to
    /// version its asset URLs, so changed bytes answer a different URL without a
    /// changed published name. A declaration this inventory does not publish is
    /// an error rather than an invented URL. `site.origin` plays no part.
    pub(crate) fn asset_urls(&self, config: &ResolvedSiteConfig) -> Result<super::AssetUrls> {
        let mount = config.url_mount();
        let mut pairs = Vec::with_capacity(self.generation.assets.len());
        for asset in self.generation.assets.iter() {
            let Some(declared_url) = asset.spec.declared_url.as_ref() else {
                // A tree publishes each member it owns under its prefix, so an entry's
                // output path is the site-root URL that member is declared under.
                // The member this URL publishes is the path inside the tree, so dropping the
                // prefix's own output path from the entry's output leaves it — the inverse of the
                // prefix's `output_for`, which the check view mirrors by walking the tree.
                let prefix = config
                    .assets
                    .trees
                    .iter()
                    .find(|declaration| {
                        declaration.source() == asset.spec.source_id.logical_source.as_path()
                    })
                    .and_then(|declaration| declaration.url_prefix().output_root())
                    .map(|root| format!("{}/", root.as_str()))
                    .unwrap_or_default();
                pairs.extend(asset.entries.iter().map(|(entry, _)| {
                    let url = tola_address::asset_url_from_output(&entry.output);
                    let browser = super::urls::browser_url(&mount, &url, entry.identity());
                    let member = entry
                        .output
                        .as_str()
                        .strip_prefix(prefix.as_str())
                        .unwrap_or(entry.output.as_str());
                    (
                        url.as_str().to_owned(),
                        browser,
                        super::AssetOrigin::TreeMember {
                            source: asset.spec.source_id.logical_source.clone(),
                            member: std::path::PathBuf::from(member),
                        },
                    )
                }));
                continue;
            };
            let [(entry, _)] = asset.entries.as_ref() else {
                anyhow::bail!(
                    "asset `{}` declared in `assets.files` did not publish exactly one file, so its URL cannot be resolved",
                    crate::filesystem::display_path(
                        asset.spec.source_id.logical_source.as_path(),
                        config.get_root()
                    )
                );
            };
            pairs.push((
                declared_url.clone(),
                super::urls::browser_url(
                    &mount,
                    &tola_address::asset_url_from_output(&entry.output),
                    entry.identity(),
                ),
                super::AssetOrigin::File {
                    source: asset.spec.source_id.logical_source.clone(),
                },
            ));
        }
        Ok(super::AssetUrls::from_published(pairs))
    }

    pub(crate) fn watch_evidence(&self) -> FilesystemWatchEvidence {
        FilesystemWatchEvidence::from_sources(
            self.generation
                .assets
                .iter()
                .map(|asset| (asset.spec.source_id.kind, asset.source_identity.clone())),
        )
    }

    /// Recheck the exact raw input evidence without rendering asset bytes again.
    pub(crate) fn is_fresh_for(
        &self,
        config: &ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<bool> {
        cancellation.ensure_active()?;
        let observed = (|| -> Result<bool> {
            if self.generation.spec != ConfiguredAssetSpec::for_config(config) {
                return Ok(false);
            }
            let mut verified = std::collections::BTreeSet::new();
            for asset in self.generation.assets.iter() {
                cancellation.ensure_active()?;
                let resolved = resolve_asset_source(config, &asset.spec.source_id, cancellation)?;
                if asset.source_identity != resolved.source_identity {
                    return Ok(false);
                }
                if verified.insert(resolved.observation_key()) {
                    // An empty change list requires complete membership and byte observation;
                    // retained evidence only lets unchanged raw allocations be shared.
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
        // Missing, unreadable, or retargeted inputs invalidate the candidate. A
        // cancelled observation remains cancellation, never a stale-input verdict.
        cancellation.ensure_active()?;
        Ok(observed.unwrap_or(false))
    }
}

/// The configuration key that declares a source of this kind.
fn declaration_key(kind: FilesystemSourceKind) -> &'static str {
    match kind {
        FilesystemSourceKind::Tree => "assets.trees",
        FilesystemSourceKind::File => "assets.files",
    }
}

/// Reader noun for one member of a configured asset tree.
const TREE_MEMBER_LABEL: &str = "`assets.trees` member";

/// The sources exact `assets.files` declarations publish.
///
/// A declaration owns the output of the source it names, so a tree that also
/// covers that source publishes every other member and skips this one. The
/// build applies this to a tree's observation and the read-only check applies
/// it while enumerating, so both agree on which members a tree publishes.
///
/// Ownership follows source identity — logical path, or canonical path of an alias —
/// never declaration order; it affects that source's static mapping, not conversion or exclusion.
#[derive(Debug, Default)]
struct DeclaredFileSources {
    logical: std::collections::BTreeSet<PathBuf>,
    canonical: std::collections::BTreeSet<PathBuf>,
}

impl DeclaredFileSources {
    fn for_config(config: &ResolvedSiteConfig) -> Self {
        let mut sources = Self::default();
        for declaration in &config.assets.files {
            let logical = crate::filesystem::lexical_path_identity(declaration.source());
            if let Some(canonical) = FilesystemSourceIdentity::from_path(&logical).canonical_path()
            {
                sources.canonical.insert(canonical.to_path_buf());
            }
            sources.logical.insert(logical);
        }
        sources
    }

    fn is_empty(&self) -> bool {
        self.logical.is_empty()
    }

    /// Whether one exact declaration publishes the source at these identities.
    ///
    /// `canonical` is the member's physical identity, so a member reached
    /// through an alias of a declared source is that declared source.
    fn owns(&self, logical: &Path, canonical: Option<&Path>) -> bool {
        self.logical.contains(logical)
            || canonical.is_some_and(|canonical| self.canonical.contains(canonical))
    }
}

/// Whether a tree publishes the member at `relative` inside its own source.
fn tree_publishes_member(
    declared: &DeclaredFileSources,
    logical_root: &Path,
    canonical_root: Option<&Path>,
    relative: &Path,
) -> bool {
    let logical = logical_root.join(relative);
    let canonical = canonical_root.map(|root| root.join(relative));
    !declared.owns(&logical, canonical.as_deref())
}

/// Enumerate the members one tree declaration publishes, without reading them.
///
/// The read-only sibling of observing a tree: the same membership rules, the
/// same exclusion of sources an exact declaration publishes, and no bytes,
/// rendering, or minification. A tree source that is missing, unreadable, or
/// outside its allowed boundary is an error here exactly as it is for a build.
pub(super) fn enumerate_tree_members(
    config: &ResolvedSiteConfig,
    declaration: &crate::config::section::AssetTreeDeclaration,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<PathBuf>> {
    let id = ConfiguredSourceId {
        kind: FilesystemSourceKind::Tree,
        logical_source: absolute_path(declaration.source()),
    };
    let resolved = resolve_asset_source(config, &id, cancellation)?;
    let declared = DeclaredFileSources::for_config(config);
    let display_root = config.get_root();
    let logical_root = resolved.source_identity.logical_path();
    let canonical_root = resolved
        .source_identity
        .canonical_path()
        .map(Path::to_path_buf);
    let mut members = Vec::new();
    crate::filesystem::walk_tree_members(
        &resolved.source_identity,
        cancellation,
        display_root,
        TREE_MEMBER_LABEL,
        &|relative, logical_source| {
            validate_tree_member_output_path(relative, logical_source, display_root)
        },
        &mut |_, relative| {
            if tree_publishes_member(&declared, logical_root, canonical_root.as_deref(), relative) {
                members.push(relative.to_path_buf());
            }
            Ok(())
        },
    )?;
    Ok(members)
}

fn resolve_asset_source(
    config: &ResolvedSiteConfig,
    id: &ConfiguredSourceId,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<ResolvedAssetSource> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let source = crate::filesystem::display_path(&id.logical_source, config.get_root());
    let key = declaration_key(id.kind);
    let metadata = match std::fs::metadata(&id.logical_source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "asset source `{source}` declared in `{key}` does not exist; create the file or fix the declaration"
        ),
        Err(error) => {
            anyhow::bail!(
                "cannot read asset source `{source}` declared in `{key}`: {}; check that the file is readable",
                crate::filesystem::path_failure_reason(&error)
            );
        }
    };
    let (correct_kind, expected) = match id.kind {
        FilesystemSourceKind::Tree => (metadata.is_dir(), "directory"),
        FilesystemSourceKind::File => (metadata.is_file(), "regular file"),
    };
    if !correct_kind {
        anyhow::bail!(
            "`{key}` source `{source}` is not a {expected}; fix the declaration in `tola.toml`"
        );
    }
    let source_identity = FilesystemSourceIdentity::canonical(
        crate::filesystem::lexical_path_identity(&id.logical_source),
        std::fs::canonicalize(&id.logical_source).map_err(|error| {
            anyhow::anyhow!(
                "cannot read asset source `{source}` declared in `{key}`: {}; check that the file is readable",
                crate::filesystem::path_failure_reason(&error)
            )
        })?,
    );
    validate_asset_source_boundary(config, &source_identity)?;
    Ok(ResolvedAssetSource {
        id: id.clone(),
        source_identity,
    })
}

fn validate_asset_source_boundary(
    config: &ResolvedSiteConfig,
    source: &FilesystemSourceIdentity,
) -> Result<()> {
    let internal = config.get_root().join(crate::filesystem::INTERNAL_DIR);
    for (owner, protected) in [
        (
            "content directory (`build.content-dir`)",
            config.build.content_dir.as_path(),
        ),
        (
            "output directory (`build.publish-dir`)",
            config.build.publish_dir.as_path(),
        ),
        ("site's private `.tola` directory", internal.as_path()),
    ] {
        let protected = FilesystemSourceIdentity::canonical(
            crate::filesystem::lexical_path_identity(protected),
            crate::filesystem::normalize_existing_prefix(protected),
        );
        if source.intersects(&protected) {
            anyhow::bail!(
                "asset source `{}` overlaps the {owner}; choose a source outside it",
                crate::filesystem::display_path(source.logical_path(), config.get_root())
            );
        }
    }
    crate::resources::source_boundary(config, crate::InputScope::Online)
        .check(source.logical_path())?;
    Ok(())
}

/// Update configured assets without enumerating unaffected sources.
pub(crate) fn render_configured_asset_inventory(
    config: &ResolvedSiteConfig,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &crate::cancellation::BuildCancellation,
    previous: Option<&ConfiguredAssetInventory>,
    changed_paths: &[PathBuf],
) -> Result<ConfiguredAssetInventory> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let spec = ConfiguredAssetSpec::for_config(config);
    for asset in &spec.assets {
        let identity = FilesystemSourceIdentity::from_path(&asset.source_id.logical_source);
        validate_asset_source_boundary(config, &identity)?;
        boundary.check(&asset.source_id.logical_source)?;
    }
    let previous = previous.filter(|previous| &previous.generation.boundary == boundary);
    if previous.is_none_or(|previous| previous.generation.spec != spec) {
        validate_asset_output_ownership(&spec)?;
    }
    if let Some(previous) = previous
        && previous.generation.spec == spec
        && !previous
            .generation
            .assets
            .iter()
            .any(|asset| asset.source_identity.changed_paths_intersect(changed_paths))
    {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        for asset in previous.generation.assets.iter() {
            validate_asset_source_boundary(config, &asset.source_identity)?;
        }
        return Ok(previous.clone());
    }

    let mut source_ids = spec
        .assets
        .iter()
        .map(|declaration| &declaration.source_id)
        .collect::<Vec<_>>();
    // Declaration ordering groups each logical source before its output fanout.
    source_ids.dedup();
    let previous_identities = previous
        .into_iter()
        .flat_map(|inventory| inventory.generation.assets.iter())
        .map(|asset| (&asset.spec.source_id, &asset.source_identity))
        .collect::<BTreeMap<_, _>>();
    let resolved_results = source_ids
        .par_iter()
        .map(|id| {
            cancellation.ensure_active().map_err(anyhow::Error::new)?;
            if let Some(&source_identity) = previous_identities.get(id)
                && !source_identity.changed_paths_intersect(changed_paths)
            {
                validate_asset_source_boundary(config, source_identity)?;
                return Ok(ResolvedAssetSource {
                    id: (*id).clone(),
                    source_identity: source_identity.clone(),
                });
            }
            resolve_asset_source(config, id, cancellation)
        })
        .collect::<Vec<_>>();
    let resolved_sources = resolved_results.into_iter().collect::<Result<Vec<_>>>()?;
    validate_source_ownership(config, &resolved_sources)?;

    let previous_observations = previous
        .into_iter()
        .flat_map(|inventory| inventory.generation.assets.iter())
        .map(|asset| {
            (
                AssetObservationKey::for_source(asset.spec.source_id.kind, &asset.source_identity),
                Arc::clone(&asset.observation),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let previous_assets = previous
        .into_iter()
        .flat_map(|inventory| inventory.generation.assets.iter())
        .map(|asset| (&asset.spec, asset))
        .collect::<BTreeMap<_, _>>();
    let mut minification = AssetMinification::default();
    if let Some(previous) = previous {
        seed_minify_cache(previous, &mut minification);
    }
    let mut observation_sources = BTreeMap::<AssetObservationKey, Vec<&ResolvedAssetSource>>::new();
    for resolved in &resolved_sources {
        observation_sources
            .entry(resolved.observation_key())
            .or_default()
            .push(resolved);
    }
    let observation_requests = observation_sources.into_iter().collect::<Vec<_>>();
    let observe = |(key, sources): (AssetObservationKey, Vec<&ResolvedAssetSource>)| -> Result<_> {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let previous_observation = previous_observations.get(&key);
        let changed_source = sources.iter().copied().find(|source| {
            source
                .source_identity
                .changed_paths_intersect(changed_paths)
        });
        let observation = match (previous_observation, changed_source) {
            (Some(previous), None) => Arc::clone(previous),
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
    let observation_results = observation_requests
        .into_par_iter()
        .map(observe)
        .collect::<Vec<_>>();
    let observations = observation_results
        .into_iter()
        .collect::<Result<BTreeMap<_, _>>>()?;

    let resolved_by_id = resolved_sources
        .into_iter()
        .map(|resolved| (resolved.id, resolved.source_identity))
        .collect::<BTreeMap<_, _>>();
    let mut assets = Vec::with_capacity(spec.assets.len());
    for asset in &spec.assets {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let source_identity = resolved_by_id
            .get(&asset.source_id)
            .expect("every declaration has a resolved source")
            .clone();
        let observation = Arc::clone(
            observations
                .get(&AssetObservationKey::for_source(
                    asset.source_id.kind,
                    &source_identity,
                ))
                .expect("every resolved source has one observation"),
        );
        let previous_asset = previous_assets.get(asset).copied();
        let (entries, minify_warnings) = if let Some(previous_asset) = previous_asset
            && previous.is_some_and(|previous| previous.generation.spec.publishes_like(&spec))
            && previous_asset.source_identity == source_identity
            && Arc::ptr_eq(&previous_asset.observation, &observation)
        {
            (
                Arc::clone(&previous_asset.entries),
                Arc::clone(&previous_asset.minify_warnings),
            )
        } else {
            let entries = render_asset_entries(
                config,
                asset,
                &observation,
                spec.minify,
                spec.cache_busting,
                &mut minification,
                cancellation,
            )?;
            (entries.into(), minification.take_warnings().into())
        };
        assets.push(ConfiguredAssetEntry {
            spec: asset.clone(),
            source_identity,
            observation,
            entries,
            minify_warnings,
        });
    }
    Ok(ConfiguredAssetInventory {
        generation: Arc::new(ConfiguredAssetGeneration {
            spec,
            assets: assets.into(),
            boundary: boundary.clone(),
        }),
    })
}

fn observe_source(
    config: &ResolvedSiteConfig,
    boundary: &tola_typst::SourceBoundary,
    id: &ConfiguredSourceId,
    source_identity: &FilesystemSourceIdentity,
    cancellation: &crate::cancellation::BuildCancellation,
    previous: Option<&FilesystemSourceObservation>,
    changed_paths: &[PathBuf],
) -> Result<FilesystemSourceObservation> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let display_root = config.get_root();
    match id.kind {
        FilesystemSourceKind::Tree => {
            let observation = crate::filesystem::observe_tree_source(
                source_identity,
                boundary,
                previous,
                changed_paths,
                cancellation,
                display_root,
                TREE_MEMBER_LABEL,
                &|relative, logical_source| {
                    validate_tree_member_output_path(relative, logical_source, display_root)
                },
            )?;
            // The tree publishes every member of its source except the sources an exact
            // declaration already publishes; those bytes keep the declaration's address.
            let declared = DeclaredFileSources::for_config(config);
            if declared.is_empty() {
                return Ok(observation);
            }
            let logical_root = source_identity.logical_path();
            let canonical_root = source_identity.canonical_path().map(Path::to_path_buf);
            Ok(observation.retaining(|file| {
                tree_publishes_member(
                    &declared,
                    logical_root,
                    canonical_root.as_deref(),
                    file.relative_path(),
                )
            }))
        }
        FilesystemSourceKind::File => crate::filesystem::observe_file_source(
            source_identity,
            boundary,
            previous,
            cancellation,
            display_root,
        ),
    }
}

fn validate_tree_member_output_path(
    relative: &Path,
    logical_source: &Path,
    display_root: &Path,
) -> Result<()> {
    let member = crate::filesystem::display_path(logical_source, display_root);
    for component in relative.components() {
        let std::path::Component::Normal(segment) = component else {
            anyhow::bail!(
                "{TREE_MEMBER_LABEL} `{member}` cannot be published: its path must contain only file and directory names; rename the file or directory"
            );
        };
        let Some(segment) = segment.to_str() else {
            anyhow::bail!(
                "{TREE_MEMBER_LABEL} `{member}` cannot be published because its name is not valid UTF-8; rename the file or directory"
            );
        };
        if segment.contains(super::output::DECLARED_URL_SYNTAX) {
            anyhow::bail!(
                "{TREE_MEMBER_LABEL} `{member}` cannot be published because its path contains `?`, `#`, or `%`; rename the file or directory"
            );
        }
    }
    Ok(())
}

fn render_asset_entries(
    config: &ResolvedSiteConfig,
    spec: &ConfiguredAsset,
    observation: &FilesystemSourceObservation,
    languages: MinifiedLanguages,
    cache_busting: bool,
    minification: &mut AssetMinification,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<(AssetOutput, Arc<[u8]>)>> {
    let display_root = config.get_root();
    let mut entries = Vec::with_capacity(observation.files().len());
    for source in observation.files() {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let source_path = asset_source_path(spec, source);
        let display_source = crate::filesystem::display_path(&source_path, display_root);
        let bytes = minification.rendered_bytes(
            &source_path,
            &display_source,
            source.digest(),
            source.bytes(),
            languages,
        );
        // The identity names the bytes Tola publishes, so it is taken from the rendered
        // output, not the raw input a minifier may rewrite. It rides in the URL
        // `asset-url()` resolves; the published path stays what the declaration said.
        let identity =
            cache_busting.then(|| Arc::<str>::from(tola_typst::ContentDigest::of(&bytes).to_hex()));
        let output = output_for_asset_source(spec, source, &source_path, display_root, identity)?;
        entries.push((output, bytes));
    }
    validate_asset_output_set(
        entries.iter().map(|(output, _)| output),
        display_root,
        cancellation,
    )?;
    Ok(entries)
}

fn seed_minify_cache(inventory: &ConfiguredAssetInventory, minification: &mut AssetMinification) {
    for asset in inventory.generation.assets.iter() {
        for (source, (_, bytes)) in asset.observation.files().iter().zip(asset.entries.iter()) {
            let source_path = asset_source_path(&asset.spec, source);
            minification.seed(
                AssetMinifyKey::for_source(
                    &source_path,
                    source.digest(),
                    inventory.generation.spec.minify,
                ),
                Arc::clone(bytes),
            );
        }
    }
}

fn asset_source_path(spec: &ConfiguredAsset, source: &FilesystemSourceFile) -> PathBuf {
    spec.source_id.logical_source.join(source.relative_path())
}

fn output_for_asset_source(
    spec: &ConfiguredAsset,
    source: &FilesystemSourceFile,
    logical_source: &Path,
    display_root: &Path,
    identity: Option<Arc<str>>,
) -> Result<AssetOutput> {
    let mapped = match &spec.output {
        ConfiguredAssetOutput::TreePrefix(url_prefix) => {
            super::output::output_for_configured_tree_member(
                logical_source,
                source.relative_path(),
                url_prefix,
                display_root,
            )?
        }
        ConfiguredAssetOutput::File { output } => {
            super::output::output_for_configured_file(&spec.source_id.logical_source, output)
        }
    };
    // A `files` declaration and a `trees` member are both published files, so
    // both hold the identity of the bytes a build rendered for them.
    Ok(mapped.with_identity(identity))
}

fn validate_source_ownership(
    config: &ResolvedSiteConfig,
    sources: &[ResolvedAssetSource],
) -> Result<()> {
    for (index, left) in sources.iter().enumerate() {
        for right in sources.iter().skip(index + 1) {
            let same_observation = left.id.kind == right.id.kind
                && (left.source_identity.logical_path() == right.source_identity.logical_path()
                    || left.source_identity.canonical_path()
                        == right.source_identity.canonical_path());
            if same_observation {
                continue;
            }
            // Only two trees compete for one source: an exact declaration owns the
            // output of the source it names, so a tree never publishes it.
            let overlaps = left.id.kind == FilesystemSourceKind::Tree
                && right.id.kind == FilesystemSourceKind::Tree
                && left.source_identity.intersects(&right.source_identity);
            if overlaps {
                anyhow::bail!(
                    "asset source `{}` declared in `{}` overlaps `{}` declared in `{}`; remove the nested source or choose separate sources",
                    crate::filesystem::display_path(
                        left.id.logical_source.as_path(),
                        config.get_root()
                    ),
                    declaration_key(left.id.kind),
                    crate::filesystem::display_path(
                        right.id.logical_source.as_path(),
                        config.get_root()
                    ),
                    declaration_key(right.id.kind)
                );
            }
        }
    }
    Ok(())
}

/// Refuse one published path with two owners, before any source is enumerated.
///
/// Only declarations of equal specificity compete. An exact `assets.files`
/// declaration owns the output of the source it names, so a tree that also
/// covers that source is not a conflict: the tree publishes every other member
/// and the declaration keeps its own address. Two trees publishing below
/// overlapping prefixes, and two exact declarations claiming one path, both
/// leave one path with two owners. So does a tree prefix below an exact file
/// URL: a file cannot also be the directory that holds the tree's members.
fn validate_asset_output_ownership(spec: &ConfiguredAssetSpec) -> Result<()> {
    let assets = spec
        .assets
        .iter()
        .map(|asset| {
            let key = asset.output.portable_key();
            (asset, key)
        })
        .collect::<Vec<_>>();
    for (index, (left, left_key)) in assets.iter().enumerate() {
        for (right, right_key) in assets.iter().skip(index + 1) {
            let conflicts = match (left.source_id.kind, right.source_id.kind) {
                (FilesystemSourceKind::Tree, FilesystemSourceKind::Tree)
                | (FilesystemSourceKind::File, FilesystemSourceKind::File) => {
                    key_is_prefix(left_key, right_key) || key_is_prefix(right_key, left_key)
                }
                (FilesystemSourceKind::File, FilesystemSourceKind::Tree) => {
                    key_is_prefix(left_key, right_key)
                }
                (FilesystemSourceKind::Tree, FilesystemSourceKind::File) => {
                    key_is_prefix(right_key, left_key)
                }
            };
            if conflicts {
                anyhow::bail!(
                    "asset output `{}` declared in `{}` overlaps output `{}` declared in `{}`; give each published path one owner",
                    left.output.display_coordinate(),
                    declaration_key(left.source_id.kind),
                    right.output.display_coordinate(),
                    declaration_key(right.source_id.kind)
                );
            }
        }
    }
    Ok(())
}

fn key_is_prefix(prefix: &[String], path: &[String]) -> bool {
    prefix.len() <= path.len() && path.starts_with(prefix)
}

fn validate_asset_output_set<'output>(
    outputs: impl IntoIterator<Item = &'output AssetOutput>,
    display_root: &Path,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<()> {
    let mut ordered = outputs
        .into_iter()
        .map(|output| (output, output.output.portable_key()))
        .collect::<Vec<_>>();
    ordered.sort_by(|(_, left), (_, right)| left.cmp(right));
    for pair in ordered.windows(2) {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let [(output, output_key), (conflict, conflict_key)] = pair else {
            unreachable!("windows(2) always yields two outputs");
        };
        if key_is_prefix(output_key, conflict_key) || key_is_prefix(conflict_key, output_key) {
            anyhow::bail!(
                "asset `{}` at `{}` conflicts with asset `{}` at `{}`; rename one so the paths do not overlap",
                crate::filesystem::display_path(&output.logical_source, display_root),
                output.output,
                crate::filesystem::display_path(&conflict.logical_source, display_root),
                conflict.output,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use tempfile::TempDir;

    use super::*;
    use crate::asset::tests::{file_declaration, published_url};
    use crate::config::section::assets::{
        AssetFileDeclaration, AssetTreeDeclaration, AssetUrlPrefix,
    };

    fn tree_declaration(
        source: impl Into<PathBuf>,
        prefix: impl AsRef<str>,
    ) -> AssetTreeDeclaration {
        AssetTreeDeclaration::new(source, AssetUrlPrefix::parse(prefix.as_ref()).unwrap())
    }

    fn render_inventory(
        config: &ResolvedSiteConfig,
        previous: Option<&ConfiguredAssetInventory>,
        changed: &[PathBuf],
    ) -> ConfiguredAssetInventory {
        render_configured_asset_inventory(
            config,
            &crate::resources::source_boundary(config, crate::InputScope::Online),
            &crate::cancellation::BuildCancellation::default(),
            previous,
            changed,
        )
        .unwrap()
    }

    fn render_inventory_error(
        config: &ResolvedSiteConfig,
        previous: Option<&ConfiguredAssetInventory>,
        changed: &[PathBuf],
    ) -> anyhow::Error {
        render_configured_asset_inventory(
            config,
            &crate::resources::source_boundary(config, crate::InputScope::Online),
            &crate::cancellation::BuildCancellation::default(),
            previous,
            changed,
        )
        .unwrap_err()
    }

    fn source_evidence<'a>(
        inventory: &'a ConfiguredAssetInventory,
        logical: &Path,
    ) -> &'a FilesystemSourceFile {
        inventory
            .generation
            .assets
            .iter()
            .find_map(|asset| {
                asset.observation.files().iter().find(|source| {
                    asset
                        .spec
                        .source_id
                        .logical_source
                        .join(source.relative_path())
                        == logical
                })
            })
            .expect("configured source evidence")
    }

    fn asset_for_source<'a>(
        inventory: &'a ConfiguredAssetInventory,
        logical: &Path,
    ) -> &'a ConfiguredAssetEntry {
        inventory
            .generation
            .assets
            .iter()
            .find(|asset| asset.spec.source_id.logical_source == logical)
            .expect("configured asset asset")
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
        let previous = rendered(&config);
        assert_eq!(only_entry(&previous).1.as_ref(), b"body { color: red; }");
        let pure = crate::resources::source_boundary(&config, crate::InputScope::Pure);
        assert!(
            render_configured_asset_inventory(&config, &pure, &cancellation, Some(&previous), &[])
                .is_err()
        );
    }

    #[test]
    fn rendering_assets_writes_no_output() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("app.css"), "body { color: red; }").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.trees = vec![tree_declaration(assets, "/assets")];
        config.build.publish_dir = directory.path().join("public");

        let inventory = rendered(&config);

        let (_, bytes) = inventory.entries().next().expect("rendered asset");
        assert!(!bytes.is_empty());
        assert!(!config.build.publish_dir.exists());
    }

    #[test]
    fn missing_declared_source_fails_discovery() {
        let directory = TempDir::new().unwrap();

        let mut tree_config = crate::config::tests::load_test_config(directory.path(), "");
        tree_config.assets.trees = vec![tree_declaration(
            directory.path().join("missing-tree"),
            "/assets",
        )];
        let tree_error = render_inventory_error(&tree_config, None, &[]);
        assert!(tree_error.to_string().contains("does not exist"));
        assert!(tree_error.to_string().contains("missing-tree"));

        let mut file_config = crate::config::tests::load_test_config(directory.path(), "");
        file_config.assets.files = vec![file_declaration(
            directory.path().join("missing-file"),
            "/favicon.ico",
        )];
        let file_error = render_inventory_error(&file_config, None, &[]);
        assert!(file_error.to_string().contains("does not exist"));
        assert!(file_error.to_string().contains("missing-file"));
    }

    /// The site directories a tree may not resolve into, with the diagnostic
    /// fragment each is rejected by.
    fn exclusive_site_roots(directory: &TempDir) -> [(PathBuf, &'static str); 3] {
        let content = directory.path().join("content");
        let output = directory.path().join("public");
        let internal = directory.path().join(crate::filesystem::INTERNAL_DIR);
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
        let error = render_inventory_error(&config, None, &[]);
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
        let published = rendered(&config);

        fs::remove_file(&output_link).unwrap();
        symlink(&assets, &output_link).unwrap();
        let error = render_inventory_error(
            &config,
            Some(&published),
            std::slice::from_ref(&output_link),
        );

        assert!(error.to_string().contains("output directory"), "{error:#}");
    }

    /// Each shape of tree member Tola cannot publish, with the diagnostic fragments that
    /// name the member and the reason.
    #[cfg(unix)]
    #[test]
    fn unpublishable_tree_members_are_rejected() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        for (label, expected) in [
            ("live.sock", &["assets/live.sock", "not a regular file"][..]),
            ("file-link", &["file-link", "symbolic link"][..]),
            ("directory-link", &["directory-link", "symbolic link"][..]),
            ("missing-link", &["missing-link", "symbolic link"][..]),
            (
                "query?name.txt",
                &[
                    "query?name.txt",
                    "cannot be published",
                    "`assets.trees` member",
                    "rename the file or directory",
                ][..],
            ),
            (
                "fragment#name.txt",
                &[
                    "fragment#name.txt",
                    "cannot be published",
                    "`assets.trees` member",
                    "rename the file or directory",
                ][..],
            ),
            (
                "percent%name.txt",
                &[
                    "percent%name.txt",
                    "cannot be published",
                    "`assets.trees` member",
                    "rename the file or directory",
                ][..],
            ),
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

            let message = render_inventory_error(&config, None, &[]).to_string();

            for fragment in expected {
                assert!(message.contains(fragment), "{label}: {message}");
            }
            if label.ends_with(".txt") {
                let checked = super::super::AssetUrls::for_check(
                    &config,
                    &crate::cancellation::BuildCancellation::default(),
                )
                .unwrap_err()
                .to_string();
                assert!(checked.contains(label), "{checked}");
                assert!(checked.contains("`assets.trees` member"), "{checked}");
                assert!(
                    checked.contains("rename the file or directory"),
                    "{checked}"
                );
            }
        }

        // An exact `assets.files` declaration refuses the same shape.
        let directory = TempDir::new().unwrap();
        let socket = directory.path().join("live.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.files = vec![file_declaration(&socket, "/live.sock")];
        let message = render_inventory_error(&config, None, &[]).to_string();
        assert!(message.contains("not a regular file"), "{message}");
        assert!(message.contains("live.sock"), "{message}");
    }

    #[test]
    fn cancellation_survives_asset_rendering() {
        let directory = TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        let error = render_configured_asset_inventory(
            &config,
            &crate::resources::source_boundary(&config, crate::InputScope::Online),
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

    /// A change to one declaration's source publishes new bytes for it and leaves
    /// every other declaration's observation and entries shared.
    #[test]
    fn changed_source_preserves_other_assets() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        let tree_source = assets.join("app.bin");
        fs::write(&tree_source, b"tree-v1").unwrap();
        let stable_assets = directory.path().join("stable-assets");
        fs::create_dir_all(&stable_assets).unwrap();
        fs::write(stable_assets.join("logo.bin"), b"stable-tree").unwrap();
        let stable_file = directory.path().join("robots.bin");
        fs::write(&stable_file, b"stable-file").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![
            tree_declaration(&assets, "/assets"),
            tree_declaration(&stable_assets, "/stable"),
        ];
        config.assets.files = vec![file_declaration(&stable_file, "/robots.bin")];
        let first = rendered(&config);
        let tree_before = asset_for_source(&first, &absolute_path(&stable_assets));
        let file_before = asset_for_source(&first, &absolute_path(&stable_file));

        fs::write(&tree_source, b"tree-v2").unwrap();
        let updated = render_inventory(&config, Some(&first), std::slice::from_ref(&tree_source));

        assert!(!Arc::ptr_eq(&first.generation, &updated.generation));

        for (before, after) in [
            (
                tree_before,
                asset_for_source(&updated, &absolute_path(&stable_assets)),
            ),
            (
                file_before,
                asset_for_source(&updated, &absolute_path(&stable_file)),
            ),
        ] {
            assert!(Arc::ptr_eq(&before.observation, &after.observation));
            assert!(Arc::ptr_eq(&before.entries, &after.entries));
        }
        assert_eq!(
            updated
                .entries()
                .find(|(output, _)| output.logical_source == absolute_path(&tree_source))
                .map(|(_, bytes)| bytes.as_ref()),
            Some(b"tree-v2".as_slice())
        );
    }

    #[test]
    fn incremental_scan_matches_cold_scan() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        let changed = assets.join("app.css");
        fs::write(&changed, "body { color: red; }").unwrap();
        for index in 0..999 {
            fs::write(
                assets.join(format!("asset-{index:04}.bin")),
                index.to_string(),
            )
            .unwrap();
        }

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&assets, "/assets")];
        let first = rendered(&config);
        let first_bytes = first
            .entries()
            .map(|(output, bytes)| (output.logical_source.clone(), Arc::clone(bytes)))
            .collect::<BTreeMap<_, _>>();

        fs::write(&changed, "body { color: blue; }").unwrap();
        let incremental = render_inventory(&config, Some(&first), std::slice::from_ref(&changed));

        let authoritative = rendered(&config);
        assert_eq!(
            incremental.entries().count(),
            authoritative.entries().count()
        );
        for ((incremental_output, incremental_bytes), (expected_output, expected_bytes)) in
            incremental.entries().zip(authoritative.entries())
        {
            assert_eq!(
                incremental_output.logical_source,
                expected_output.logical_source
            );
            assert_eq!(incremental_output.output, expected_output.output);
            assert_eq!(incremental_bytes, expected_bytes);
            if incremental_output.logical_source != absolute_path(&changed) {
                assert!(Arc::ptr_eq(
                    first_bytes
                        .get(&incremental_output.logical_source)
                        .expect("previous configured asset bytes"),
                    incremental_bytes
                ));
            }
        }

        assert!(
            incremental
                .is_fresh_for(&config, &crate::cancellation::BuildCancellation::default())
                .unwrap()
        );
    }

    #[test]
    fn policy_reuses_source_observations() {
        let directory = TempDir::new().unwrap();
        let css = directory.path().join("app.css");
        let javascript = directory.path().join("app.js");
        let module = directory.path().join("app.mjs");
        let minified = directory.path().join("already.min.css");
        let css_source = ".a { color: red; } .b { color: red; }";
        let javascript_source = "function answer() { return 6 * 7; }";
        let module_source = "export const answer = 6 * 7;";
        let minified_source = ".kept { padding: 1px; }";
        fs::write(&css, css_source).unwrap();
        fs::write(&javascript, javascript_source).unwrap();
        fs::write(&module, module_source).unwrap();
        fs::write(&minified, minified_source).unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.files = vec![
            file_declaration(&css, "/app.css"),
            file_declaration(&javascript, "/app.js"),
            file_declaration(&module, "/app.mjs"),
            file_declaration(&minified, "/already.min.css"),
        ];
        config.build.minify.css = false;
        config.build.minify.javascript = false;
        let raw = rendered(&config);
        let raw_entries = raw.entries().collect::<Vec<_>>();
        assert_eq!(raw_entries[0].1.as_ref(), minified_source.as_bytes());
        assert_eq!(raw_entries[1].1.as_ref(), css_source.as_bytes());
        assert_eq!(raw_entries[2].1.as_ref(), javascript_source.as_bytes());
        assert_eq!(raw_entries[3].1.as_ref(), module_source.as_bytes());

        config.build.minify.css = true;
        config.build.minify.javascript = true;
        let transformed = render_inventory(&config, Some(&raw), &[]);
        assert!(!Arc::ptr_eq(&raw.generation, &transformed.generation));
        for (before, after) in raw
            .generation
            .assets
            .iter()
            .zip(transformed.generation.assets.iter())
        {
            assert!(Arc::ptr_eq(&before.observation, &after.observation));
        }
        let transformed_entries = transformed.entries().collect::<Vec<_>>();
        assert_eq!(
            transformed_entries[0].1.as_ref(),
            minified_source.as_bytes()
        );
        let transformed_css = std::str::from_utf8(&transformed_entries[1].1).unwrap();
        assert!(
            transformed_css.len() < css_source.len(),
            "{transformed_css}"
        );
        assert!(
            transformed_css.contains(".a") && transformed_css.contains(".b"),
            "{transformed_css}"
        );
        let javascript = std::str::from_utf8(&transformed_entries[2].1).unwrap();
        assert!(javascript.contains("answer"), "{javascript}");
        assert!(javascript.len() < javascript_source.len());
        let module = std::str::from_utf8(&transformed_entries[3].1).unwrap();
        assert!(module.contains("export"), "{module}");
        assert!(module.contains("answer"), "{module}");
        let css_derived = Arc::clone(&transformed_entries[1].1);
        assert_eq!(
            source_evidence(&transformed, &absolute_path(&css))
                .bytes()
                .as_ref(),
            css_source.as_bytes(),
            "source observation must retain raw bytes"
        );

        config.build.minify.javascript = false;
        let css_only = render_inventory(&config, Some(&transformed), &[]);
        let css_only_entries = css_only.entries().collect::<Vec<_>>();
        assert!(Arc::ptr_eq(&css_derived, &css_only_entries[1].1));
        assert_eq!(css_only_entries[2].1.as_ref(), javascript_source.as_bytes());
        assert_eq!(css_only_entries[3].1.as_ref(), module_source.as_bytes());

        config.build.minify.css = false;
        let restored = render_inventory(&config, Some(&css_only), &[]);
        let restored_entries = restored.entries().collect::<Vec<_>>();
        assert_eq!(restored_entries[1].1.as_ref(), css_source.as_bytes());
        assert_eq!(restored_entries[2].1.as_ref(), javascript_source.as_bytes());
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
            let inventory = rendered(&config);

            let warnings = inventory.minify_warnings();
            let [warning] = &warnings[..] else {
                panic!("expected one warning for {name}, got {warnings:?}");
            };
            assert!(
                matches!(warning, AssetMinifyWarning::Syntax { path, .. } if path == name),
                "{warning:?}"
            );
            let published = inventory
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
    fn one_tree_publishes_under_two_prefixes() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("app.css"), b".a { color: red; }").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.build.minify.css = true;
        config.assets.trees = vec![
            tree_declaration(&assets, "/first"),
            tree_declaration(&assets, "/second"),
        ];

        let inventory = rendered(&config);

        let entries = inventory.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0.output.as_str(), "first/app.css");
        assert_eq!(entries[1].0.output.as_str(), "second/app.css");
        assert!(entries[0].1.len() < b".a { color: red; }".len());
        assert!(std::str::from_utf8(&entries[0].1).unwrap().contains(".a"));
        assert!(Arc::ptr_eq(&entries[0].1, &entries[1].1));
        assert!(Arc::ptr_eq(
            &inventory.generation.assets[0].observation,
            &inventory.generation.assets[1].observation
        ));
    }

    #[test]
    fn declaration_reorder_keeps_the_generation() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("download.js");
        fs::write(&source, b"function download() { return 6 * 7; }").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.minify.javascript = true;
        config.assets.files = vec![
            file_declaration(&source, "/first.bin"),
            file_declaration(&source, "/second.bin"),
        ];
        let first = rendered(&config);
        let first_entries = first.entries().collect::<Vec<_>>();
        assert_eq!(first_entries.len(), 2);
        assert!(Arc::ptr_eq(
            &first.generation.assets[0].observation,
            &first.generation.assets[1].observation
        ));
        assert!(Arc::ptr_eq(&first_entries[0].1, &first_entries[1].1));

        config.assets.files.reverse();
        let reordered =
            render_inventory(&config, Some(&first), &[directory.path().join("tola.toml")]);
        assert!(Arc::ptr_eq(&first.generation, &reordered.generation));
    }

    #[test]
    fn overlapping_asset_outputs_are_rejected() {
        let directory = TempDir::new().unwrap();
        let tree = directory.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("member.bin"), b"tree").unwrap();
        let first_file = directory.path().join("first.bin");
        let second_file = directory.path().join("second.bin");
        fs::write(&first_file, b"first").unwrap();
        fs::write(&second_file, b"second").unwrap();

        let mut files = crate::config::tests::load_test_config(directory.path(), "");
        files.assets.files = vec![
            file_declaration(&first_file, "/download"),
            file_declaration(&second_file, "/download/readme.txt"),
        ];
        let error = render_inventory_error(&files, None, &[]);
        assert!(error.to_string().contains("overlaps output"), "{error:#}");

        for (tree_prefix, file_url) in [("/downloads/icons", "/downloads"), ("/assets", "/assets")]
        {
            let mut config = crate::config::tests::load_test_config(directory.path(), "");
            config.assets.trees = vec![tree_declaration(&tree, tree_prefix)];
            config.assets.files = vec![file_declaration(&first_file, file_url)];
            let error = render_inventory_error(&config, None, &[]);
            assert!(error.to_string().contains("overlaps output"), "{error:#}");
        }
    }

    #[test]
    fn exact_declaration_and_tree_share_prefix() {
        let directory = TempDir::new().unwrap();
        let tree = directory.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("member.bin"), b"tree").unwrap();
        let file = directory.path().join("logo.svg");
        fs::write(&file, b"<svg></svg>").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&tree, "/assets")];
        config.assets.files = vec![file_declaration(&file, "/assets/logo.svg")];

        let inventory = rendered(&config);

        // The exact declaration owns its own URL, and the tree keeps the members
        // it publishes, so one prefix holds both without a second owner.
        assert_eq!(
            published_outputs(&inventory),
            ["assets/logo.svg", "assets/member.bin"]
        );
        let urls = inventory.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&urls, "/assets/logo.svg").as_deref(),
            Some("/assets/logo.svg")
        );
        assert_eq!(
            published_url(&urls, "/assets/member.bin").as_deref(),
            Some("/assets/member.bin")
        );
    }

    #[cfg(unix)]
    #[test]
    fn aliased_trees_share_one_observation() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let physical = directory.path().join("physical-assets");
        let alias = directory.path().join("asset-alias");
        fs::create_dir_all(&physical).unwrap();
        fs::write(physical.join("app.css"), b".a { color: red; }").unwrap();
        symlink(&physical, &alias).unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.build.minify.css = true;
        config.assets.trees = vec![
            tree_declaration(&physical, "/physical"),
            tree_declaration(&alias, "/alias"),
        ];

        let inventory = rendered(&config);

        let entries = inventory.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert!(Arc::ptr_eq(
            &inventory.generation.assets[0].observation,
            &inventory.generation.assets[1].observation
        ));
        assert!(Arc::ptr_eq(&entries[0].1, &entries[1].1));
        assert!(entries[0].1.len() < b".a { color: red; }".len());
        assert!(std::str::from_utf8(&entries[0].1).unwrap().contains(".a"));
        assert!(inventory.entries().any(|(output, _)| {
            output.logical_source == absolute_path(&alias).join("app.css")
                && output.output.as_str() == "alias/app.css"
        }));
        assert!(inventory.entries().any(|(output, _)| {
            output.logical_source == absolute_path(&physical).join("app.css")
                && output.output.as_str() == "physical/app.css"
        }));
    }

    #[test]
    fn unrelated_edit_reuses_frozen_inventory() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        let source = assets.join("app.bin");
        fs::write(&source, b"published-v1").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&assets, "/assets")];
        let first = rendered(&config);

        fs::write(&source, b"unobserved-v2").unwrap();
        fs::write(assets.join("unobserved.bin"), b"unobserved").unwrap();
        let unrelated = directory.path().join("content/post.typ");
        let reused = render_inventory(&config, Some(&first), std::slice::from_ref(&unrelated));

        assert!(Arc::ptr_eq(&first.generation, &reused.generation));
        let (_, reused_bytes) = reused.entries().next().expect("reused configured asset");
        assert_eq!(reused.entries().count(), 1);
        assert_eq!(reused_bytes.as_ref(), b"published-v1");

        let delta = render_inventory(&config, Some(&reused), std::slice::from_ref(&source));
        assert!(!Arc::ptr_eq(&reused.generation, &delta.generation));
        assert_eq!(delta.entries().count(), 1);
        assert!(delta.entries().any(|(output, bytes)| output.logical_source
            == absolute_path(&source)
            && bytes.as_ref() == b"unobserved-v2"));
        assert!(
            !delta
                .is_fresh_for(&config, &crate::cancellation::BuildCancellation::default())
                .unwrap(),
            "the write gate must reject unreported membership"
        );

        let authoritative = rendered(&config);
        assert_eq!(authoritative.entries().count(), 2);
    }

    #[test]
    fn ancestor_change_rescans_the_inventory() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("a b.css"), b".a { color: red; }").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.build.minify.css = true;
        config.assets.trees = vec![tree_declaration(&assets, "/图")];
        let first = rendered(&config);

        let ancestor = directory.path().to_path_buf();
        let rescanned = render_inventory(&config, Some(&first), std::slice::from_ref(&ancestor));
        assert!(!Arc::ptr_eq(&first.generation, &rescanned.generation));
        let derived_before = Arc::clone(
            &rescanned
                .entries()
                .next()
                .expect("rescanned configured asset")
                .1,
        );

        config.assets.trees = vec![tree_declaration(&assets, "/静态")];
        let unrelated = directory.path().join("content/post.typ");
        let remapped =
            render_inventory(&config, Some(&rescanned), std::slice::from_ref(&unrelated));
        assert!(!Arc::ptr_eq(&rescanned.generation, &remapped.generation));
        assert!(Arc::ptr_eq(
            &rescanned.generation.assets[0].observation,
            &remapped.generation.assets[0].observation
        ));
        let (output, bytes) = remapped
            .entries()
            .next()
            .expect("remapped configured asset");
        assert!(Arc::ptr_eq(&derived_before, bytes));
        assert_eq!(output.output.as_str(), "静态/a b.css");
    }

    #[test]
    fn tree_membership_changes_are_tracked() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("static/web-assets");
        fs::create_dir_all(&assets).unwrap();
        let first_path = assets.join("first.bin");
        let second_path = assets.join("second.bin");
        fs::write(&first_path, b"first").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&assets, "/")];
        let first = rendered(&config);

        fs::write(&second_path, b"second").unwrap();
        let added = render_inventory(&config, Some(&first), std::slice::from_ref(&second_path));
        assert_eq!(added.entries().count(), 2);

        fs::remove_file(&first_path).unwrap();
        let removed = render_inventory(&config, Some(&added), std::slice::from_ref(&first_path));
        let (output, bytes) = removed.entries().next().expect("retained configured asset");
        assert_eq!(removed.entries().count(), 1);
        assert_eq!(output.logical_source, absolute_path(&second_path));
        assert_eq!(bytes.as_ref(), b"second");
    }

    #[test]
    fn freshness_requires_unchanged_inputs() {
        let directory = TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        fs::create_dir_all(&assets).unwrap();
        let source = assets.join("app.bin");
        let added = assets.join("added.bin");
        fs::write(&source, b"candidate-bytes").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&assets, "/assets")];
        let fresh = || crate::cancellation::BuildCancellation::default();
        let inventory = rendered(&config);

        assert!(inventory.is_fresh_for(&config, &fresh()).unwrap());

        fs::write(&source, b"mutated-after-candidate").unwrap();
        assert!(!inventory.is_fresh_for(&config, &fresh()).unwrap());
        fs::write(&source, b"candidate-bytes").unwrap();

        fs::write(&added, b"candidate-bytes").unwrap();
        assert!(
            !inventory.is_fresh_for(&config, &fresh()).unwrap(),
            "an added member changes the observed inventory"
        );
        fs::remove_file(&added).unwrap();
        assert!(inventory.is_fresh_for(&config, &fresh()).unwrap());

        fs::remove_file(&source).unwrap();
        assert!(
            !inventory.is_fresh_for(&config, &fresh()).unwrap(),
            "a removed source invalidates the retained inventory"
        );
        fs::write(&source, b"candidate-bytes").unwrap();

        config.assets.trees = vec![tree_declaration(&assets, "/static")];
        assert!(
            !inventory.is_fresh_for(&config, &fresh()).unwrap(),
            "a changed output mapping invalidates the retained inventory"
        );
    }

    #[test]
    fn cancelled_freshness_is_cancellation() {
        let directory = TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let canceller = crate::cancellation::BuildCanceller::default();
        let cancellation = canceller.token();
        let inventory = render_configured_asset_inventory(
            &config,
            &crate::resources::source_boundary(&config, crate::InputScope::Online),
            &cancellation,
            None,
            &[],
        )
        .unwrap();
        canceller.cancel();

        let error = inventory.is_fresh_for(&config, &cancellation).unwrap_err();

        assert!(
            error
                .downcast_ref::<crate::cancellation::BuildCancelled>()
                .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_symlink_keeps_logical_extension() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let physical = directory.path().join("script-source.txt");
        let logical = directory.path().join("app.js");
        let source =
            "function inlineHandler() { return crossScriptValue; } var crossScriptValue = 42;";
        fs::write(&physical, source).unwrap();
        symlink(&physical, &logical).unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.files = vec![file_declaration(&logical, "/app.js")];
        config.build.minify.javascript = true;

        let inventory = rendered(&config);
        let entries = inventory.entries().collect::<Vec<_>>();
        let rendered = std::str::from_utf8(&entries[0].1).unwrap();

        assert!(rendered.len() < source.len(), "{rendered}");
        assert!(rendered.contains("inlineHandler"), "{rendered}");
        assert!(rendered.contains("crossScriptValue"), "{rendered}");
        assert_eq!(entries[0].0.logical_source, absolute_path(&logical));
        let evidence = source_evidence(&inventory, &absolute_path(&logical));
        assert_eq!(evidence.bytes().as_ref(), source.as_bytes());
        assert_eq!(
            evidence.canonical_target(),
            fs::canonicalize(&physical).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_file_symlink_is_stale() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let first = directory.path().join("asset-a.bin");
        let second = directory.path().join("asset-b.bin");
        let logical = directory.path().join("asset.bin");
        fs::write(&first, b"same bytes").unwrap();
        fs::write(&second, b"same bytes").unwrap();
        symlink(&first, &logical).unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.files = vec![file_declaration(&logical, "/asset.bin")];
        let inventory = rendered(&config);

        let logical_identity = absolute_path(&logical);
        assert_eq!(
            Some(source_evidence(&inventory, &logical_identity).canonical_target()),
            Some(fs::canonicalize(&first).unwrap().as_path())
        );
        assert!(
            inventory
                .is_fresh_for(&config, &crate::cancellation::BuildCancellation::default())
                .unwrap()
        );
        fs::remove_file(&logical).unwrap();
        symlink(&second, &logical).unwrap();
        assert!(
            !inventory
                .is_fresh_for(&config, &crate::cancellation::BuildCancellation::default())
                .unwrap()
        );

        let rebuilt = render_inventory(&config, Some(&inventory), std::slice::from_ref(&logical));
        assert!(!Arc::ptr_eq(&inventory.generation, &rebuilt.generation));
        assert_eq!(
            Some(source_evidence(&rebuilt, &logical_identity).canonical_target()),
            Some(fs::canonicalize(&second).unwrap().as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_tree_symlink_is_stale() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let first = directory.path().join("asset-tree-a");
        let second = directory.path().join("asset-tree-b");
        let logical = directory.path().join("assets");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("app.bin"), b"same bytes").unwrap();
        fs::write(second.join("app.bin"), b"same bytes").unwrap();
        symlink(&first, &logical).unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&logical, "/assets")];
        let inventory = rendered(&config);

        let logical_source = absolute_path(&logical).join("app.bin");
        assert_eq!(
            Some(source_evidence(&inventory, &logical_source).canonical_target()),
            Some(fs::canonicalize(first.join("app.bin")).unwrap().as_path())
        );
        assert!(
            inventory
                .is_fresh_for(&config, &crate::cancellation::BuildCancellation::default())
                .unwrap()
        );
        fs::remove_file(&logical).unwrap();
        symlink(&second, &logical).unwrap();
        assert!(
            !inventory
                .is_fresh_for(&config, &crate::cancellation::BuildCancellation::default())
                .unwrap()
        );
    }

    fn declared_file_inventory(
        directory: &Path,
        source: &Path,
        declaration: AssetFileDeclaration,
        settings: &str,
    ) -> (ResolvedSiteConfig, ConfiguredAssetInventory) {
        let mut config = crate::config::tests::load_test_config(directory, settings);
        config.build.publish_dir = directory.join("public");
        assert_eq!(declaration.source(), source);
        config.assets.files = vec![declaration];
        let inventory = rendered(&config);
        (config, inventory)
    }

    fn only_entry(inventory: &ConfiguredAssetInventory) -> (&AssetOutput, &Arc<[u8]>) {
        let (output, bytes) = inventory.entries().next().expect("one published asset");
        assert_eq!(inventory.entries().count(), 1);
        (output, bytes)
    }

    fn check_urls(config: &ResolvedSiteConfig) -> super::super::AssetUrls {
        super::super::AssetUrls::for_check(
            config,
            &crate::cancellation::BuildCancellation::default(),
        )
        .expect("configured declarations resolve without rendering")
    }

    #[test]
    fn configured_address_needs_no_identity() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("app.js");
        fs::write(&source, "const answer = 40 + 2;").unwrap();
        let (config, inventory) = declared_file_inventory(
            directory.path(),
            &source,
            file_declaration(&source, "/app.js"),
            "[site]\nbase-path = \"/blog/\"",
        );

        let (output, _) = only_entry(&inventory);

        assert_eq!(output.output.as_str(), "app.js");
        assert_eq!(output.identity(), None);
        let urls = inventory.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&urls, "/app.js").as_deref(),
            Some("/blog/app.js")
        );
        assert_eq!(urls, check_urls(&config));
    }

    #[test]
    fn published_address_has_the_identity() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("app.js");
        fs::write(&source, "const answer = 40 + 2;").unwrap();
        let (config, first) = declared_file_inventory(
            directory.path(),
            &source,
            file_declaration(&source, "/app.js"),
            "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
        );

        let (output, bytes) = only_entry(&first);
        let identity = tola_typst::ContentDigest::of(bytes).to_hex();
        assert_eq!(output.output.as_str(), "app.js");
        assert_eq!(output.identity(), Some(identity.as_str()));
        let urls = first.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&urls, "/app.js").as_deref(),
            Some(format!("/blog/app.js?h={identity}").as_str())
        );

        let unchanged = render_inventory(&config, Some(&first), std::slice::from_ref(&source));
        assert_eq!(unchanged.asset_urls(&config).unwrap(), urls);

        fs::write(&source, "const answer = 40 + 3;").unwrap();
        let changed = render_inventory(&config, Some(&unchanged), std::slice::from_ref(&source));
        let (changed_output, changed_bytes) = only_entry(&changed);
        let changed_identity = tola_typst::ContentDigest::of(changed_bytes).to_hex();
        assert_ne!(changed_identity, identity);
        assert_eq!(changed_output.output.as_str(), "app.js");
        let changed_urls = changed.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&changed_urls, "/app.js").as_deref(),
            Some(format!("/blog/app.js?h={changed_identity}").as_str())
        );
        assert!(!urls.values_match(&changed_urls));
    }

    #[test]
    fn cache_busting_identifies_every_file() {
        let (directory, mut config) =
            tree_site("[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"");
        let declared = directory.path().join("assets/theme.css");
        config.assets.files = vec![file_declaration(&declared, "/site.css")];

        let inventory = rendered(&config);
        assert_eq!(
            published_outputs(&inventory),
            ["assets/app.js", "assets/images/logo.svg", "site.css"]
        );
        let urls = inventory.asset_urls(&config).unwrap();
        let identity_of = |name: &str| {
            let (_, bytes) = inventory
                .entries()
                .find(|(output, _)| output.output.as_str() == name)
                .unwrap_or_else(|| panic!("{name} is published"));
            tola_typst::ContentDigest::of(bytes).to_hex()
        };
        for (declared, published) in [
            ("/site.css", "site.css"),
            ("/assets/app.js", "assets/app.js"),
            ("/assets/images/logo.svg", "assets/images/logo.svg"),
        ] {
            assert_eq!(
                published_url(&urls, declared).as_deref(),
                Some(format!("/blog/{published}?h={}", identity_of(published)).as_str()),
                "{declared}"
            );
        }

        let checked = check_urls(&config);
        for (declared, mounted) in [
            ("/site.css", "/blog/site.css"),
            ("/assets/app.js", "/blog/assets/app.js"),
            ("/assets/images/logo.svg", "/blog/assets/images/logo.svg"),
        ] {
            assert_eq!(published_url(&checked, declared).as_deref(), Some(mounted));
        }
    }

    #[test]
    fn identity_follows_published_bytes() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("app.js");
        let raw = "function  add (a, b) {\n  return a + b;\n}\n";
        fs::write(&source, raw).unwrap();
        let (config, inventory) = declared_file_inventory(
            directory.path(),
            &source,
            file_declaration(&source, "/app.js"),
            "[assets]\ncache-busting = true\n[build.minify]\njavascript = true",
        );

        let (_, bytes) = only_entry(&inventory);

        assert_ne!(bytes.as_ref(), raw.as_bytes());
        let published = tola_typst::ContentDigest::of(bytes).to_hex();
        assert_eq!(
            published_url(&inventory.asset_urls(&config).unwrap(), "/app.js").as_deref(),
            Some(format!("/app.js?h={published}").as_str())
        );
        assert_ne!(
            published,
            tola_typst::ContentDigest::of(raw.as_bytes()).to_hex(),
            "the identity follows the bytes Tola publishes, after minification"
        );
    }

    #[test]
    fn two_declarations_conflict_on_one_output() {
        let directory = TempDir::new().unwrap();
        let tree = directory.path().join("assets");
        let elsewhere = directory.path().join("elsewhere");
        fs::create_dir_all(&tree).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(tree.join("app.js"), "/* tree member */").unwrap();
        let source = elsewhere.join("app.js");
        fs::write(&source, "/* declared file */").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.publish_dir = directory.path().join("public");
        config.assets.trees = vec![tree_declaration(&tree, "/assets")];
        config.assets.files = vec![file_declaration(&source, "/assets/app.js")];

        let inventory = rendered(&config);
        assert_eq!(inventory.entries().count(), 2);

        let mut outputs = crate::output::graph::OutputGraphBuilder::new();
        let mut conflict = None;
        for (asset, bytes) in inventory.entries() {
            if let Err(error) = outputs.insert_configured_asset(
                asset.logical_source.clone(),
                asset.output.clone(),
                asset.declaration().clone(),
                Arc::clone(bytes),
            ) {
                conflict = Some(error.to_string());
            }
        }

        let conflict = conflict.expect("two owners of one output path must conflict");
        assert!(conflict.contains("assets/app.js"), "{conflict}");
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

    fn rendered(config: &ResolvedSiteConfig) -> ConfiguredAssetInventory {
        render_inventory(config, None, &[])
    }

    fn published_outputs(inventory: &ConfiguredAssetInventory) -> Vec<String> {
        let mut outputs = inventory
            .entries()
            .map(|(output, _)| output.output.as_str().to_owned())
            .collect::<Vec<_>>();
        outputs.sort();
        outputs
    }

    #[test]
    fn tree_members_resolve_under_their_prefix() {
        let (_directory, config) = tree_site("[site]\nbase-path = \"/blog/\"");
        let inventory = rendered(&config);

        assert_eq!(
            published_outputs(&inventory),
            [
                "assets/app.js",
                "assets/images/logo.svg",
                "assets/theme.css"
            ]
        );
        let urls = inventory.asset_urls(&config).unwrap();
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
        assert_eq!(urls, checked);
    }

    #[test]
    fn checks_enumerate_members_without_bytes() {
        let (_directory, mut config) = tree_site("");
        config.build.minify.css = true;
        fs::write(config.get_root().join("assets/broken.css"), "@media (").unwrap();

        let inventory = rendered(&config);
        assert!(
            inventory.minify_warnings().iter().any(|warning| matches!(
                warning,
                AssetMinifyWarning::Syntax { path, .. } if path.ends_with("broken.css")
            )),
            "rendering reports the source it could not minify"
        );

        let checked = check_urls(&config);

        assert_eq!(
            published_url(&checked, "/assets/broken.css").as_deref(),
            Some("/assets/broken.css")
        );
    }

    #[test]
    fn exact_declaration_removes_tree_member() {
        let (_directory, mut config) = tree_site("[site]\nbase-path = \"/blog/\"");
        config.assets.files = vec![
            file_declaration(config.get_root().join("assets/app.js"), "/assets/app.js"),
            file_declaration(config.get_root().join("assets/theme.css"), "/js/theme.css"),
        ];

        let inventory = rendered(&config);

        assert_eq!(
            published_outputs(&inventory),
            ["assets/app.js", "assets/images/logo.svg", "js/theme.css"]
        );
        let tree = asset_for_source(&inventory, &absolute_path(config.assets.trees[0].source()));
        assert_eq!(
            tree.entries
                .iter()
                .map(|(output, _)| output.output.as_str())
                .collect::<Vec<_>>(),
            ["assets/images/logo.svg"]
        );

        let urls = inventory.asset_urls(&config).unwrap();
        assert_eq!(
            published_url(&urls, "/assets/app.js").as_deref(),
            Some("/blog/assets/app.js")
        );
        assert_eq!(
            published_url(&urls, "/js/theme.css").as_deref(),
            Some("/blog/js/theme.css")
        );
        assert_eq!(
            published_url(&urls, "/assets/images/logo.svg").as_deref(),
            Some("/blog/assets/images/logo.svg")
        );
        assert_eq!(published_url(&urls, "/assets/theme.css").as_deref(), None);

        let checked = check_urls(&config);
        for declared in ["/assets/app.js", "/js/theme.css", "/assets/images/logo.svg"] {
            assert_eq!(
                published_url(&checked, declared),
                published_url(&urls, declared)
            );
        }
        assert_eq!(
            published_url(&checked, "/assets/theme.css").as_deref(),
            None
        );
    }

    #[test]
    fn unrelated_declarations_keep_addresses() {
        let (directory, mut config) = tree_site("[assets]\ncache-busting = true");
        config.assets.files = vec![file_declaration(
            directory.path().join("assets/theme.css"),
            "/site.css",
        )];
        let first = rendered(&config);
        let before = first.asset_urls(&config).unwrap();
        let app = published_url(&before, "/assets/app.js")
            .expect("the tree publishes app.js")
            .to_owned();
        assert!(app.contains("?h="), "{app}");

        let added = config.get_root().join("assets/added.svg");
        fs::write(&added, b"<svg></svg>").unwrap();
        let grown = render_inventory(&config, Some(&first), std::slice::from_ref(&added));
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
        let narrowed = rendered(&config);
        let narrowed_urls = narrowed.asset_urls(&config).unwrap();
        assert_eq!(published_url(&narrowed_urls, "/site.css").as_deref(), None);
        assert_eq!(
            published_url(&narrowed_urls, "/assets/app.js").as_deref(),
            Some(app.as_str())
        );
        assert!(before.values_match(&narrowed_urls));
    }
}
