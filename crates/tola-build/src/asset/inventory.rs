//! Configured assets: resolving declared sources, observing and rendering their bytes.

#![expect(
    clippy::mutable_key_type,
    reason = "FileSelection compares immutable patterns, never the matcher's internal cache"
)]

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::filesystem::{FilesystemSourceKind, FilesystemSourceObservation, absolute_path};
use crate::observation::{InputKind, InputObservation, InputPathScope};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tola_input::{FileSelection, FilesystemSourceIdentity};

use rayon::prelude::*;
use tola_minify::MinifiedLanguages;

use super::minify::{AssetMinification, AssetMinifyKey, AssetMinifyWarning};
use super::output::{AssetOutput, AssetOutputError};

/// Why one configured asset source cannot be resolved or published.
#[derive(Debug, thiserror::Error)]
enum AssetSourceError {
    /// A declared source does not exist.
    #[error(
        "asset source `{path}` declared in `{key}` does not exist; create the file or fix the \
         declaration"
    )]
    SourceMissing { path: String, key: &'static str },
    /// A declared source cannot be read.
    #[error(
        "cannot read asset source `{path}` declared in `{key}`: {reason}; check that the file is \
         readable"
    )]
    SourceUnreadable {
        path: String,
        key: &'static str,
        reason: &'static str,
    },
    /// A declared source has the wrong kind for its declaration.
    #[error("`{key}` source `{path}` is not a {expected}; fix the declaration")]
    SourceKind {
        key: &'static str,
        path: String,
        expected: &'static str,
    },
    /// A module asset source points outside its input root.
    #[error("asset source `{path}` escapes the {owner} root")]
    SourceEscapes { path: String, owner: &'static str },
    /// A declared source overlaps a directory Tola owns.
    #[error("asset source `{path}` overlaps the {owner}; choose a source outside it")]
    OverlapsProtected { path: String, owner: &'static str },
    /// Two nested tree sources publish overlapping members.
    #[error(
        "asset source `{left}` declared in `{left_key}` overlaps `{right}` declared in \
         `{right_key}`; remove the nested source or choose separate sources"
    )]
    TreeSourcesOverlap {
        left: String,
        left_key: &'static str,
        right: String,
        right_key: &'static str,
    },
    /// Two declarations publish one output path.
    #[error(
        "asset output `{left}` declared in `{left_key}` overlaps output `{right}` declared in \
         `{right_key}`; give each published path one owner"
    )]
    OutputsOverlap {
        left: String,
        left_key: &'static str,
        right: String,
        right_key: &'static str,
    },
    /// Two published paths overlap.
    #[error(
        "asset `{left}` at `{left_at}` conflicts with asset `{right}` at `{right_at}`; rename one \
         so the paths do not overlap"
    )]
    PublishedPathsConflict {
        left: String,
        left_at: String,
        right: String,
        right_at: String,
    },
}

/// Immutable configured assets shared across clones and unchanged rebuilds.
#[derive(Clone, Debug)]
pub(crate) struct ConfiguredAssetInventory {
    generation: Arc<ConfiguredAssetGeneration>,
}

impl ConfiguredAssetInventory {
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
    assets: Vec<RenderedAsset>,
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

    /// What this generation offers the next render.
    fn reusable(&self) -> ReusableAssets<'_> {
        let mut reusable = ReusableAssets::default();
        for asset in &self.assets {
            reusable.rendered.insert(&asset.declaration, asset);
            reusable
                .sources
                .insert(&asset.declaration.source_id, &asset.source_identity);
            reusable.observations.insert(
                AssetObservationKey::for_source(
                    &asset.declaration.source_id,
                    &asset.source_identity,
                ),
                &asset.observation,
            );
        }
        reusable
    }

    /// Seed the next render with published bytes so a source is minified once.
    fn seed_minification(&self, config: &ResolvedSiteConfig, minification: &mut AssetMinification) {
        for asset in &self.assets {
            for (source, (_, bytes)) in asset.observation.files().iter().zip(asset.outputs.iter()) {
                let source_path = asset
                    .declaration
                    .source_id
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
struct RenderedAsset {
    declaration: AssetDeclaration,
    source_identity: FilesystemSourceIdentity,
    observation: Arc<FilesystemSourceObservation>,
    /// One rendered output per published file, in observation order.
    outputs: Arc<[(AssetOutput, Arc<[u8]>)]>,
    /// What this source's minification could not do, kept with the bytes it published.
    minify_warnings: Arc<[AssetMinifyWarning]>,
}

/// What a previous generation offers this render: rendered assets, sources, observations.
#[derive(Default)]
struct ReusableAssets<'a> {
    rendered: BTreeMap<&'a AssetDeclaration, &'a RenderedAsset>,
    sources: BTreeMap<&'a ConfiguredSourceId, &'a FilesystemSourceIdentity>,
    observations: BTreeMap<AssetObservationKey, &'a Arc<FilesystemSourceObservation>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ConfiguredSourceId {
    kind: FilesystemSourceKind,
    logical_source: PathBuf,
    owner: Option<String>,
    within: Option<PathBuf>,
    exact_files: DeclaredFileSources,
    selection: Option<(PathBuf, Arc<FileSelection>)>,
}

impl ConfiguredSourceId {
    fn selects_tree_member(
        &self,
        source: &FilesystemSourceIdentity,
        relative: &Path,
        directory: bool,
    ) -> bool {
        let (source_prefix, selection) = self.selection.as_ref().expect("trees have a selection");
        if relative.as_os_str().is_empty() {
            return selection.could_match_below(source_prefix);
        }
        let path = source_prefix.join(relative);
        if directory {
            selection.could_match_below(&path)
        } else {
            selection.matches(&path) && self.exact_files.publishes(source, relative)
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AssetDeclaration {
    source_id: ConfiguredSourceId,
    output: ConfiguredAssetOutput,
    mount: Option<tola_address::OutputPath>,
    module_input: Option<super::urls::ModuleAssetInput>,
    minify: bool,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ConfiguredAssetOutput {
    TreePrefix(crate::config::section::AssetUrlPrefix),
    File {
        output: tola_address::OutputPath,
        /// A declaration such as `/docs/` names the file `docs/index.html`.
        declared_url: String,
    },
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
            Self::File { output, .. } => output.portable_key(),
        }
    }

    fn display_coordinate(&self) -> String {
        match self {
            Self::TreePrefix(url_prefix) => url_prefix.to_string(),
            Self::File { output, .. } => format!("/{output}"),
        }
    }
}

impl AssetDeclaration {
    fn minified_languages(&self, languages: MinifiedLanguages) -> MinifiedLanguages {
        if self.minify {
            languages
        } else {
            MinifiedLanguages::new(false, false)
        }
    }

    fn portable_key(&self) -> Vec<String> {
        let mut key = self
            .mount
            .as_ref()
            .map_or_else(Vec::new, |path| path.portable_key());
        key.extend(self.output.portable_key());
        key
    }

    fn display_coordinate(&self) -> String {
        match &self.mount {
            Some(mount) => format!("/{mount}{}", self.output.display_coordinate()),
            None => self.output.display_coordinate(),
        }
    }

    fn output(
        &self,
        relative: &Path,
        logical_source: &Path,
        display_root: &Path,
        identity: Option<Arc<str>>,
    ) -> Result<AssetOutput> {
        use super::urls::{ModuleAsset, ModuleAssetInput};
        let mut mapped = match &self.output {
            ConfiguredAssetOutput::TreePrefix(url_prefix) => {
                super::output::output_for_configured_tree_member(
                    logical_source,
                    relative,
                    url_prefix,
                    display_root,
                )?
            }
            ConfiguredAssetOutput::File { output, .. } => {
                super::output::output_for_configured_file(logical_source, output)
            }
        };
        if let (Some(alias), Some(mount), Some(input)) =
            (&self.source_id.owner, &self.mount, &self.module_input)
        {
            let input = match input {
                ModuleAssetInput::Module(path) if !relative.as_os_str().is_empty() => {
                    ModuleAssetInput::Module(path.join(relative))
                }
                input => input.clone(),
            };
            mapped.module = Some(ModuleAsset {
                alias: alias.clone(),
                name: mapped.output.as_str().to_owned(),
                input,
            });
            mapped.output = mount.join(&mapped.output);
        }
        Ok(mapped.with_identity(identity))
    }

    fn published(
        &self,
        config: &ResolvedSiteConfig,
        output: &AssetOutput,
        relative: &Path,
    ) -> (
        String,
        String,
        super::AssetOrigin,
        Option<super::urls::ModuleAsset>,
    ) {
        let url = tola_address::asset_url_from_output(&output.output);
        let (declared, origin) = match &self.output {
            ConfiguredAssetOutput::TreePrefix(_) => (
                url.as_str().to_owned(),
                super::AssetOrigin::TreeMember {
                    source: self.source_id.logical_source.clone(),
                    member: relative.to_path_buf(),
                },
            ),
            ConfiguredAssetOutput::File { declared_url, .. } => (
                if self.mount.is_some() {
                    url.as_str().to_owned()
                } else {
                    declared_url.clone()
                },
                super::AssetOrigin::File {
                    source: self.source_id.logical_source.clone(),
                },
            ),
        };
        (
            declared,
            super::urls::browser_url(config.url_mount(), &url, output.identity()),
            origin,
            output.module.clone(),
        )
    }
}

fn asset_declarations(
    config: &ResolvedSiteConfig,
    assets: crate::modules::prepared::AppliedAssets<'_>,
) -> Result<Vec<AssetDeclaration>> {
    let mut declarations = Vec::new();
    append_declarations(
        &mut declarations,
        config.get_root(),
        config.get_root(),
        &config.assets.trees,
        &config.assets.files,
        &config.options,
        None,
    )?;
    for module in assets.modules {
        append_declarations(
            &mut declarations,
            &module.root,
            config.get_root(),
            &module.declarations.trees,
            &module.declarations.files,
            &module.options,
            Some(module),
        )?;
    }
    declarations.sort_unstable();
    Ok(declarations)
}

fn append_declarations(
    declarations: &mut Vec<AssetDeclaration>,
    root: &Path,
    site_root: &Path,
    trees: &[tola_module::assets::AssetTreeDeclaration],
    files: &[tola_module::assets::AssetFileDeclaration],
    options: &toml::Table,
    module: Option<&tola_module::prepared::AssetContribution>,
) -> Result<()> {
    use super::urls::ModuleAssetInput;
    use tola_module::assets::is_enabled;

    let owner = module.map(|module| module.alias.clone());
    let label = owner
        .as_ref()
        .map_or_else(|| "site".to_owned(), |alias| format!("module `{alias}`"));
    let mount = owner
        .as_ref()
        .map(|alias| tola_address::OutputPath::parse(&format!("_tola-modules/{alias}")))
        .transpose()?;
    let within = module.map(|_| root.to_path_buf());
    let mut exact_files = DeclaredFileSources::default();
    for file in files {
        exact_files.insert(root.join(file.source()));
    }

    let trees = trees
        .iter()
        .map(|tree| {
            let source_prefix = tola_input::lexical_path_identity(
                tree.source().strip_prefix(root).unwrap_or(tree.source()),
            );
            let active = is_enabled(
                tree.enable,
                tree.enabled_by.as_deref(),
                options,
                &label,
                &source_prefix,
            )?;
            Ok((tree, source_prefix, active))
        })
        .collect::<Result<Vec<_>>>()?;

    let file_declaration =
        |source: PathBuf, output, declared_url, minify, replacement: Option<&Path>| {
            let module_input = module.map(|_| match replacement {
                Some(path) => ModuleAssetInput::Project(path.to_path_buf()),
                None => ModuleAssetInput::Module(source.clone()),
            });
            let (logical_source, within) = match replacement {
                Some(path) => (site_root.join(path), Some(site_root.to_path_buf())),
                None => (root.join(&source), within.clone()),
            };
            AssetDeclaration {
                source_id: ConfiguredSourceId {
                    kind: FilesystemSourceKind::File,
                    logical_source: absolute_path(&logical_source),
                    owner: owner.clone(),
                    within,
                    exact_files: DeclaredFileSources::default(),
                    selection: None,
                },
                output: ConfiguredAssetOutput::File {
                    output,
                    declared_url,
                },
                mount: mount.clone(),
                module_input,
                minify,
            }
        };

    let mut exact_names = BTreeSet::new();
    let mut active_exact_names = BTreeSet::new();
    for file in files {
        let output = file.url().output_path();
        exact_names.insert(output.as_str());
        if is_enabled(
            file.enable,
            file.enabled_by.as_deref(),
            options,
            &label,
            file.source().strip_prefix(root).unwrap_or(file.source()),
        )? {
            active_exact_names.insert(output.as_str());
            let replacement = module.and_then(|module| module.replace.get(output.as_str()));
            declarations.push(file_declaration(
                file.source().to_path_buf(),
                output.clone(),
                file.url().as_str().to_owned(),
                file.minify,
                replacement.map(PathBuf::as_path),
            ));
        }
    }

    let mut replaced_members = vec![Vec::new(); trees.len()];
    if let Some(module) = module {
        for (name, replacement) in &module.replace {
            if active_exact_names.contains(name.as_str()) {
                continue;
            }
            let mut matched = exact_names.contains(name.as_str());
            for (index, (tree, source_prefix, active)) in trees.iter().enumerate() {
                let Some(member) = tree.url_prefix().member_for(name) else {
                    continue;
                };
                if member.is_empty() {
                    continue;
                }
                // A tree member must have a publishable spelling and retain ownership.
                let relative = Path::new(member);
                if !tree.selection().matches(&source_prefix.join(relative))
                    || super::output::member_path_segments(relative).is_err()
                {
                    continue;
                }
                if !active {
                    matched = true;
                    continue;
                }
                let source = tree.source().join(relative);
                let logical = root.join(&source);
                let identity = FilesystemSourceIdentity::from_path(&logical);
                if exact_files.owns(identity.logical_path(), identity.canonical_path())
                    || !is_tree_file(&root.join(tree.source()), relative)
                {
                    continue;
                }
                replaced_members[index].push(logical);
                matched = true;
                declarations.push(file_declaration(
                    source,
                    tola_address::OutputPath::parse(name)?,
                    format!("/{name}"),
                    tree.minify,
                    Some(replacement),
                ));
            }
            anyhow::ensure!(matched, "{label} has no asset `{name}` to replace");
        }
    }
    for ((tree, source_prefix, active), replaced) in trees.into_iter().zip(replaced_members) {
        if active {
            let mut exact_files = exact_files.clone();
            for source in replaced {
                exact_files.insert(source);
            }
            declarations.push(AssetDeclaration {
                source_id: ConfiguredSourceId {
                    kind: FilesystemSourceKind::Tree,
                    logical_source: absolute_path(&root.join(tree.source())),
                    owner: owner.clone(),
                    within: within.clone(),
                    exact_files,
                    selection: Some((source_prefix, Arc::new(tree.selection().clone()))),
                },
                output: ConfiguredAssetOutput::TreePrefix(tree.url_prefix().clone()),
                mount: mount.clone(),
                module_input: module.map(|_| ModuleAssetInput::Module(tree.source().to_path_buf())),
                minify: tree.minify,
            });
        }
    }
    Ok(())
}

/// The tree walker refuses symlinked members, including intermediate directories.
fn is_tree_file(root: &Path, member: &Path) -> bool {
    let mut path = root.to_path_buf();
    let mut components = member.components().peekable();
    while let Some(component) = components.next() {
        path.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        if components.peek().is_none() {
            return metadata.is_file();
        }
        if !metadata.is_dir() {
            return false;
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AssetObservationKey {
    kind: FilesystemSourceKind,
    owner: Option<String>,
    exact_files: DeclaredFileSources,
    selection: Option<(PathBuf, Arc<FileSelection>)>,
    canonical: PathBuf,
}

impl AssetObservationKey {
    fn for_source(id: &ConfiguredSourceId, source: &FilesystemSourceIdentity) -> Self {
        Self {
            kind: id.kind,
            owner: id.owner.clone(),
            exact_files: id.exact_files.clone(),
            selection: id.selection.clone(),
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
        AssetObservationKey::for_source(&self.id, &self.source_identity)
    }
}

impl ConfiguredAssetInventory {
    pub(crate) fn entries(&self) -> impl Iterator<Item = &(AssetOutput, Arc<[u8]>)> {
        self.generation
            .assets
            .iter()
            .flat_map(|asset| asset.outputs.iter())
    }

    /// URL values come from this generation's declarations and rendered bytes.
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

    /// The same inventory, when every source it retains still passes a fresh render's boundary.
    fn revalidated(&self, config: &ResolvedSiteConfig) -> Result<Self> {
        for asset in self.generation.assets.iter() {
            validate_asset_source_boundary(
                config,
                &asset.declaration.source_id,
                &asset.source_identity,
            )?;
        }
        Ok(self.clone())
    }

    pub(crate) fn record_watch_inputs(&self, observation: &mut InputObservation) {
        for asset in &self.generation.assets {
            let source = &asset.source_identity;
            if let Some((source_prefix, selection)) = &asset.declaration.source_id.selection {
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

    /// Recheck the exact raw input evidence without rendering asset bytes again.
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
                .serves(config, &asset_declarations(config, assets)?)
            {
                return Ok(false);
            }
            let mut verified = BTreeSet::new();
            for asset in self.generation.assets.iter() {
                cancellation.ensure_active()?;
                let resolved =
                    resolve_asset_source(config, &asset.declaration.source_id, cancellation)?;
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

/// The configuration key that declares a source of this kind.
fn declaration_key(kind: FilesystemSourceKind) -> &'static str {
    match kind {
        FilesystemSourceKind::Tree => "assets.trees",
        FilesystemSourceKind::File => "assets.files",
    }
}

/// Reader noun for one member of a configured asset tree.
const TREE_MEMBER_LABEL: &str = "`assets.trees` member";

/// Source identities taken over by exact declarations in the same owner.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
struct DeclaredFileSources {
    logical: BTreeSet<PathBuf>,
    canonical: BTreeSet<PathBuf>,
}

impl DeclaredFileSources {
    fn insert(&mut self, source: PathBuf) {
        let logical = tola_input::lexical_path_identity(&source);
        if let Some(canonical) = FilesystemSourceIdentity::from_path(&logical).canonical_path() {
            self.canonical.insert(canonical.to_path_buf());
        }
        self.logical.insert(logical);
    }

    fn owns(&self, logical: &Path, canonical: Option<&Path>) -> bool {
        self.logical.contains(logical)
            || canonical.is_some_and(|canonical| self.canonical.contains(canonical))
    }

    fn publishes(&self, root: &FilesystemSourceIdentity, relative: &Path) -> bool {
        let logical = root.logical_path().join(relative);
        let canonical = root.canonical_path().map(|root| root.join(relative));
        !self.owns(&logical, canonical.as_deref())
    }
}

/// Check declaration addresses without reading or minifying bytes.
pub(super) fn check_asset_urls(
    config: &ResolvedSiteConfig,
    assets: crate::modules::prepared::AppliedAssets<'_>,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<super::AssetUrls> {
    let declarations = asset_declarations(config, assets)?;
    validate_asset_output_ownership(&declarations)?;
    let mut published = Vec::new();
    let mut outputs = Vec::new();
    let mut trees = Vec::new();
    for declaration in &declarations {
        cancellation.ensure_active()?;
        let id = &declaration.source_id;
        let mut record = |logical: &Path, relative: &Path| -> Result<()> {
            let output = declaration.output(relative, logical, config.get_root(), None)?;
            published.push(declaration.published(config, &output, relative));
            outputs.push(output);
            Ok(())
        };
        match id.kind {
            FilesystemSourceKind::File => {
                let identity = FilesystemSourceIdentity::from_path(&id.logical_source);
                validate_asset_source_boundary(config, id, &identity)?;
                record(&id.logical_source, Path::new(""))?;
            }
            FilesystemSourceKind::Tree => {
                let source = match resolve_asset_source(config, id, cancellation) {
                    Ok(source) => source,
                    Err(error)
                        if matches!(
                            error.downcast_ref::<AssetSourceError>(),
                            Some(AssetSourceError::SourceMissing { .. })
                        ) =>
                    {
                        let identity = FilesystemSourceIdentity::from_path(&id.logical_source);
                        validate_asset_source_boundary(config, id, &identity)?;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                crate::filesystem::walk_tree_members_selected(
                    &source.source_identity,
                    cancellation,
                    config.get_root(),
                    TREE_MEMBER_LABEL,
                    &|relative, directory| {
                        id.selects_tree_member(&source.source_identity, relative, directory)
                    },
                    &|_, _| Ok(()),
                    &mut record,
                )?;
                trees.push(source);
            }
        }
    }
    validate_source_ownership(config, &trees)?;
    validate_asset_output_set(outputs.iter(), config.get_root(), cancellation)?;
    Ok(super::AssetUrls::from_preview(published))
}

fn resolve_asset_source(
    config: &ResolvedSiteConfig,
    id: &ConfiguredSourceId,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<ResolvedAssetSource> {
    cancellation.ensure_active()?;
    let source = tola_diagnostic::display_path(&id.logical_source, config.get_root());
    let key = declaration_key(id.kind);
    let display_source = source.clone();
    let unreadable = move |error: &std::io::Error| AssetSourceError::SourceUnreadable {
        path: display_source.clone(),
        key,
        reason: crate::filesystem::path_failure_reason(error),
    };
    let metadata = match std::fs::metadata(&id.logical_source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AssetSourceError::SourceMissing { path: source, key }.into());
        }
        Err(error) => return Err(unreadable(&error).into()),
    };
    let (correct_kind, expected) = match id.kind {
        FilesystemSourceKind::Tree => (metadata.is_dir(), "directory"),
        FilesystemSourceKind::File => (metadata.is_file(), "regular file"),
    };
    if !correct_kind {
        return Err(AssetSourceError::SourceKind {
            key,
            path: source,
            expected,
        }
        .into());
    }
    let source_identity = FilesystemSourceIdentity::canonical(
        tola_input::lexical_path_identity(&id.logical_source),
        std::fs::canonicalize(&id.logical_source).map_err(|error| unreadable(&error))?,
    );
    validate_asset_source_boundary(config, id, &source_identity)?;
    Ok(ResolvedAssetSource {
        id: id.clone(),
        source_identity,
    })
}

fn validate_asset_source_boundary(
    config: &ResolvedSiteConfig,
    id: &ConfiguredSourceId,
    source: &FilesystemSourceIdentity,
) -> Result<()> {
    if let Some(root) = &id.within
        && !source
            .physical_path()
            .starts_with(tola_input::normalize_existing_prefix(root))
    {
        return Err(AssetSourceError::SourceEscapes {
            path: tola_diagnostic::display_path(source.logical_path(), config.get_root()),
            owner: if root == config.get_root() {
                "site"
            } else {
                "module"
            },
        }
        .into());
    }
    let internal = config.get_root().join(tola_input::INTERNAL_DIR);
    let module_cache = tola_input::layout::modules(config.get_root());
    let from_acquired_module = id.owner.is_some()
        && id
            .within
            .as_ref()
            .is_some_and(|root| root.starts_with(&module_cache))
        && source.logical_path().starts_with(&module_cache);
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
        if protected == internal && from_acquired_module {
            continue;
        }
        let protected = FilesystemSourceIdentity::canonical(
            tola_input::lexical_path_identity(protected),
            tola_input::normalize_existing_prefix(protected),
        );
        if source.intersects(&protected) {
            return Err(AssetSourceError::OverlapsProtected {
                path: tola_diagnostic::display_path(source.logical_path(), config.get_root()),
                owner,
            }
            .into());
        }
    }
    crate::resources::source_boundary(config, tola_input::InputScope::Online)
        .check(source.logical_path())?;
    Ok(())
}

/// Update configured assets without enumerating unaffected sources.
pub(crate) fn render_configured_asset_inventory(
    config: &ResolvedSiteConfig,
    assets: crate::modules::prepared::AppliedAssets<'_>,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &crate::cancellation::BuildCancellation,
    previous: Option<&ConfiguredAssetInventory>,
    changed_paths: &[PathBuf],
) -> Result<ConfiguredAssetInventory> {
    cancellation.ensure_active()?;
    let declarations = asset_declarations(config, assets)?;
    validate_declared_sources(config, boundary, &declarations)?;
    let previous = previous.filter(|previous| &previous.generation.boundary == boundary);
    let config_unchanged =
        previous.is_some_and(|previous| previous.generation.serves(config, &declarations));
    // A generation exists only after its declarations passed this check.
    if !config_unchanged {
        validate_asset_output_ownership(&declarations)?;
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
    validate_source_ownership(config, &resolved_sources)?;
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
    let rendering_unchanged =
        previous.is_some_and(|previous| previous.generation.publishes_like(config));
    let mut assets = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        cancellation.ensure_active()?;
        let source = *resolved_by_id
            .get(&declaration.source_id)
            .expect("every declaration has a resolved source");
        let observation = Arc::clone(
            observations
                .get(&source.observation_key())
                .expect("every resolved source has one observation"),
        );
        let previous_asset = reusable.rendered.get(&declaration).copied();
        let (outputs, minify_warnings) = if let Some(previous_asset) = previous_asset
            && rendering_unchanged
            && previous_asset.source_identity == source.source_identity
            && Arc::ptr_eq(&previous_asset.observation, &observation)
        {
            (
                Arc::clone(&previous_asset.outputs),
                Arc::clone(&previous_asset.minify_warnings),
            )
        } else {
            let outputs = render_asset_outputs(
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
        assets.push(RenderedAsset {
            declaration,
            source_identity: source.source_identity.clone(),
            observation,
            outputs,
            minify_warnings,
        });
    }
    validate_asset_output_set(
        assets
            .iter()
            .flat_map(|asset| asset.outputs.iter().map(|(output, _)| output)),
        config.get_root(),
        cancellation,
    )?;
    Ok(ConfiguredAssetInventory {
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
        let identity = FilesystemSourceIdentity::from_path(&asset.source_id.logical_source);
        validate_asset_source_boundary(config, &asset.source_id, &identity)?;
        boundary.check(&asset.source_id.logical_source)?;
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
) -> Result<Vec<ResolvedAssetSource>> {
    let mut source_ids = declarations
        .iter()
        .map(|declaration| &declaration.source_id)
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
                validate_asset_source_boundary(config, id, source_identity)?;
                return Ok(ResolvedAssetSource {
                    id: (*id).clone(),
                    source_identity: source_identity.clone(),
                });
            }
            resolve_asset_source(config, id, cancellation)
        })
        .collect::<Vec<_>>();
    resolved.into_iter().collect()
}

/// Observe each distinct source once; declarations reaching one canonical source share the result.
fn observe_resolved_sources(
    config: &ResolvedSiteConfig,
    boundary: &tola_typst::SourceBoundary,
    resolved_sources: &[ResolvedAssetSource],
    reusable: &ReusableAssets<'_>,
    config_unchanged: bool,
    changed_paths: &[PathBuf],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<BTreeMap<AssetObservationKey, Arc<FilesystemSourceObservation>>> {
    let mut observation_sources = BTreeMap::<AssetObservationKey, Vec<&ResolvedAssetSource>>::new();
    for resolved in resolved_sources {
        observation_sources
            .entry(resolved.observation_key())
            .or_default()
            .push(resolved);
    }
    let observe = |(key, sources): (AssetObservationKey, Vec<&ResolvedAssetSource>)| -> Result<_> {
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

fn observe_source(
    config: &ResolvedSiteConfig,
    boundary: &tola_typst::SourceBoundary,
    id: &ConfiguredSourceId,
    source_identity: &FilesystemSourceIdentity,
    cancellation: &crate::cancellation::BuildCancellation,
    previous: Option<&FilesystemSourceObservation>,
    changed_paths: &[PathBuf],
) -> Result<FilesystemSourceObservation> {
    cancellation.ensure_active()?;
    let display_root = config.get_root();
    match id.kind {
        FilesystemSourceKind::Tree => {
            let validate = |relative: &Path, logical_source: &Path| {
                validate_tree_member_output_path(relative, logical_source, display_root)
            };
            crate::filesystem::observe_tree_source_selected(
                crate::filesystem::TreeObservation {
                    source_identity,
                    boundary,
                    previous,
                    changed_paths,
                    cancellation,
                    display_root,
                    member_label: TREE_MEMBER_LABEL,
                    validate_member: &validate,
                },
                &|relative, directory| id.selects_tree_member(source_identity, relative, directory),
            )
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
    let member = tola_diagnostic::display_path(logical_source, display_root);
    super::output::member_path_segments(relative)
        .map(|_| ())
        .map_err(|violation| {
            anyhow::Error::from(AssetOutputError::MemberPath { member, violation })
        })
}

fn render_asset_outputs(
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
        let source_path = asset.source_id.logical_source.join(source.relative_path());
        let display_source = tola_diagnostic::display_path(&source_path, display_root);
        let bytes = minification.rendered_bytes(
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

fn validate_source_ownership(
    config: &ResolvedSiteConfig,
    sources: &[ResolvedAssetSource],
) -> Result<()> {
    for (index, left) in sources.iter().enumerate() {
        for right in sources.iter().skip(index + 1) {
            if left.id.owner != right.id.owner {
                continue;
            }
            let same_observation = left.id.kind == right.id.kind
                && (left.source_identity.logical_path() == right.source_identity.logical_path()
                    || left.source_identity.canonical_path()
                        == right.source_identity.canonical_path());
            if same_observation {
                continue;
            }
            // Only two trees compete for one source; an exact declaration owns its own output.
            let overlaps = left.id.kind == FilesystemSourceKind::Tree
                && right.id.kind == FilesystemSourceKind::Tree
                && left.source_identity.intersects(&right.source_identity);
            if overlaps {
                return Err(AssetSourceError::TreeSourcesOverlap {
                    left: tola_diagnostic::display_path(
                        left.id.logical_source.as_path(),
                        config.get_root(),
                    ),
                    left_key: declaration_key(left.id.kind),
                    right: tola_diagnostic::display_path(
                        right.id.logical_source.as_path(),
                        config.get_root(),
                    ),
                    right_key: declaration_key(right.id.kind),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn validate_asset_output_ownership(declarations: &[AssetDeclaration]) -> Result<()> {
    let assets = declarations
        .iter()
        .map(|asset| {
            let key = asset.portable_key();
            (asset, key)
        })
        .collect::<Vec<_>>();
    for (index, (left, left_key)) in assets.iter().enumerate() {
        for (right, right_key) in assets.iter().skip(index + 1) {
            if left.source_id.owner != right.source_id.owner {
                continue;
            }
            let conflicts = match (&left.output, &right.output) {
                (ConfiguredAssetOutput::TreePrefix(_), ConfiguredAssetOutput::TreePrefix(_))
                | (ConfiguredAssetOutput::File { .. }, ConfiguredAssetOutput::File { .. }) => {
                    key_is_prefix(left_key, right_key) || key_is_prefix(right_key, left_key)
                }
                (ConfiguredAssetOutput::File { .. }, ConfiguredAssetOutput::TreePrefix(_)) => {
                    key_is_prefix(left_key, right_key)
                }
                (ConfiguredAssetOutput::TreePrefix(_), ConfiguredAssetOutput::File { .. }) => {
                    key_is_prefix(right_key, left_key)
                }
            };
            if conflicts {
                return Err(AssetSourceError::OutputsOverlap {
                    left: left.display_coordinate(),
                    left_key: declaration_key(left.source_id.kind),
                    right: right.display_coordinate(),
                    right_key: declaration_key(right.source_id.kind),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn key_is_prefix(prefix: &[String], path: &[String]) -> bool {
    prefix.len() <= path.len() && path.starts_with(prefix)
}

pub(super) fn validate_asset_output_set<'output>(
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
        cancellation.ensure_active()?;
        let [(output, output_key), (conflict, conflict_key)] = pair else {
            unreachable!("windows(2) always yields two outputs");
        };
        if key_is_prefix(output_key, conflict_key) || key_is_prefix(conflict_key, output_key) {
            return Err(AssetSourceError::PublishedPathsConflict {
                left: tola_diagnostic::display_path(&output.logical_source, display_root),
                left_at: output.output.to_string(),
                right: tola_diagnostic::display_path(&conflict.logical_source, display_root),
                right_at: conflict.output.to_string(),
            }
            .into());
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
        super::super::AssetUrls::for_check_with(
            config,
            crate::modules::prepared::AppliedAssets::project_only(config),
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

    fn render_inventory_result(
        config: &ResolvedSiteConfig,
        previous: Option<&ConfiguredAssetInventory>,
        changed: &[PathBuf],
    ) -> anyhow::Result<ConfiguredAssetInventory> {
        render_configured_asset_inventory(
            config,
            crate::modules::prepared::AppliedAssets::project_only(config),
            &crate::resources::source_boundary(config, tola_input::InputScope::Online),
            &crate::cancellation::BuildCancellation::default(),
            previous,
            changed,
        )
    }

    fn render_inventory(
        config: &ResolvedSiteConfig,
        previous: Option<&ConfiguredAssetInventory>,
        changed: &[PathBuf],
    ) -> ConfiguredAssetInventory {
        render_inventory_result(config, previous, changed).unwrap()
    }

    fn render_inventory_error(
        config: &ResolvedSiteConfig,
        previous: Option<&ConfiguredAssetInventory>,
        changed: &[PathBuf],
    ) -> anyhow::Error {
        render_inventory_result(config, previous, changed).unwrap_err()
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
        let pure = crate::resources::source_boundary(&config, tola_input::InputScope::Pure);
        assert!(
            render_configured_asset_inventory(
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

            let message = render_inventory_error(&config, None, &[]).to_string();

            assert!(message.contains(label), "{label}: {message}");
            assert!(message.contains(reason), "{label}: {message}");
            if label.ends_with(".txt") {
                // The read-only check refuses the same member before rendering any bytes.
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
        let urls = inventory.asset_urls(&config).unwrap();
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
    fn tree_members_use_prefix() {
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
