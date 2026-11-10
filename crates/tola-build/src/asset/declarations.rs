//! Expand site and module declarations before reading their published files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tola_input::FilesystemSourceIdentity;
use tola_minify::MinifiedLanguages;

use crate::asset::AssetOrigin;
use crate::asset::output::{self, AssetOutput};
use crate::asset::urls::{ModuleAsset, ModuleAssetInput, browser_url};
use crate::config::ResolvedSiteConfig;
use crate::filesystem::{FilesystemSourceKind, absolute_path};

use super::AssetSourceError;
use super::source::{AssetSource, DeclaredFileSources, declaration_key};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AssetDeclaration {
    pub(super) source: AssetSource,
    output: DeclaredOutput,
    mount: Option<tola_address::OutputPath>,
    module_input: Option<ModuleAssetInput>,
    minify: bool,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DeclaredOutput {
    TreePrefix(crate::config::section::AssetUrlPrefix),
    File {
        output: tola_address::OutputPath,
        /// A declaration such as `/docs/` names the file `docs/index.html`.
        declared_url: String,
    },
}

impl DeclaredOutput {
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
    pub(super) fn minified_languages(&self, languages: MinifiedLanguages) -> MinifiedLanguages {
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

    pub(super) fn output(
        &self,
        relative: &Path,
        logical_source: &Path,
        display_root: &Path,
        identity: Option<Arc<str>>,
    ) -> Result<AssetOutput> {
        let mut mapped = match &self.output {
            DeclaredOutput::TreePrefix(url_prefix) => output::output_for_configured_tree_member(
                logical_source,
                relative,
                url_prefix,
                display_root,
            )?,
            DeclaredOutput::File { output, .. } => {
                output::output_for_configured_file(logical_source, output)
            }
        };
        if let (Some(alias), Some(mount), Some(input)) =
            (&self.source.owner, &self.mount, &self.module_input)
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
        mapped.identity = identity;
        Ok(mapped)
    }

    pub(super) fn published(
        &self,
        config: &ResolvedSiteConfig,
        output: &AssetOutput,
        relative: &Path,
    ) -> (String, String, AssetOrigin, Option<ModuleAsset>) {
        let url = tola_address::asset_url_from_output(&output.output);
        let (declared, origin) = match &self.output {
            DeclaredOutput::TreePrefix(_) => (
                url.as_str().to_owned(),
                AssetOrigin::TreeMember {
                    source: self.source.logical_source.clone(),
                    member: relative.to_path_buf(),
                },
            ),
            DeclaredOutput::File { declared_url, .. } => (
                if self.mount.is_some() {
                    url.as_str().to_owned()
                } else {
                    declared_url.clone()
                },
                AssetOrigin::File {
                    source: self.source.logical_source.clone(),
                },
            ),
        };
        (
            declared,
            browser_url(config.url_mount(), &url, output.identity()),
            origin,
            output.module.clone(),
        )
    }
}

pub(super) fn collect(
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
                source: AssetSource {
                    kind: FilesystemSourceKind::File,
                    logical_source: absolute_path(&logical_source),
                    owner: owner.clone(),
                    within,
                    exact_files: DeclaredFileSources::default(),
                    selection: None,
                },
                output: DeclaredOutput::File {
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
                    || output::member_path_segments(relative).is_err()
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
                source: AssetSource {
                    kind: FilesystemSourceKind::Tree,
                    logical_source: absolute_path(&root.join(tree.source())),
                    owner: owner.clone(),
                    within: within.clone(),
                    exact_files,
                    selection: Some((source_prefix, Arc::new(tree.selection().clone()))),
                },
                output: DeclaredOutput::TreePrefix(tree.url_prefix().clone()),
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

pub(super) fn check_output_overlap(declarations: &[AssetDeclaration]) -> Result<()> {
    let assets = declarations
        .iter()
        .map(|asset| {
            let key = asset.portable_key();
            (asset, key)
        })
        .collect::<Vec<_>>();
    for (index, (left, left_key)) in assets.iter().enumerate() {
        for (right, right_key) in assets.iter().skip(index + 1) {
            if left.source.owner != right.source.owner {
                continue;
            }
            let conflicts = match (&left.output, &right.output) {
                (DeclaredOutput::TreePrefix(_), DeclaredOutput::TreePrefix(_))
                | (DeclaredOutput::File { .. }, DeclaredOutput::File { .. }) => {
                    right_key.starts_with(left_key) || left_key.starts_with(right_key)
                }
                (DeclaredOutput::File { .. }, DeclaredOutput::TreePrefix(_)) => {
                    right_key.starts_with(left_key)
                }
                (DeclaredOutput::TreePrefix(_), DeclaredOutput::File { .. }) => {
                    left_key.starts_with(right_key)
                }
            };
            if conflicts {
                return Err(AssetSourceError::OutputsOverlap {
                    left: left.display_coordinate(),
                    left_key: declaration_key(left.source.kind),
                    right: right.display_coordinate(),
                    right_key: declaration_key(right.source.kind),
                }
                .into());
            }
        }
    }
    Ok(())
}
