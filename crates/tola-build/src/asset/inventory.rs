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
                    let browser = super::urls::browser_url(mount, &url, entry.identity());
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
                    mount,
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
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::asset::tests::{file_declaration, published_url};
    use crate::config::section::assets::{AssetTreeDeclaration, AssetUrlPrefix};

    fn check_urls(config: &ResolvedSiteConfig) -> super::super::AssetUrls {
        super::super::AssetUrls::for_check(
            config,
            &crate::cancellation::BuildCancellation::default(),
        )
        .expect("configured declarations resolve without rendering")
    }

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
        let mut entries = previous.entries();
        let (_, bytes) = entries.next().expect("one published asset");
        assert_eq!(entries.count(), 0, "one published asset");
        assert_eq!(bytes.as_ref(), b"body { color: red; }");
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
        assert_eq!(
            entries[0].1, entries[1].1,
            "one source publishes one result"
        );
    }

    /// A tree publishes under its prefix; an exact declaration owns its own URL and keeps the
    /// tree from publishing that member twice; a path that would collide is refused.
    #[test]
    fn declaration_owns_url_inside_tree_prefix() {
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
        let inventory = rendered(&config);
        assert_eq!(
            published_outputs(&inventory),
            ["assets/logo.svg", "assets/member.bin"]
        );
        let urls = inventory.asset_urls(&config).unwrap();
        for declared in ["/assets/logo.svg", "/assets/member.bin"] {
            assert_eq!(published_url(&urls, declared).as_deref(), Some(declared));
        }

        // An exact declaration takes the URL away from the tree member it names.
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
        let urls = inventory.asset_urls(&config).unwrap();
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
            let error = render_inventory_error(&files, None, &[]);
            assert!(
                error.to_string().contains("overlaps output"),
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
