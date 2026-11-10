//! Source selection, filesystem boundaries, and evidence for configured assets.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tola_input::{FileSelection, FilesystemSourceIdentity};

use crate::asset::output::{self, AssetOutputError};
use crate::config::ResolvedSiteConfig;
use crate::filesystem::{FilesystemSourceKind, FilesystemSourceObservation};

use super::AssetSourceError;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AssetSource {
    pub(super) kind: FilesystemSourceKind,
    pub(super) logical_source: PathBuf,
    pub(super) owner: Option<String>,
    pub(super) within: Option<PathBuf>,
    pub(super) exact_files: DeclaredFileSources,
    pub(super) selection: Option<(PathBuf, Arc<FileSelection>)>,
}

impl AssetSource {
    pub(super) fn selects_tree_member(
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ObservationKey {
    kind: FilesystemSourceKind,
    owner: Option<String>,
    exact_files: DeclaredFileSources,
    selection: Option<(PathBuf, Arc<FileSelection>)>,
    canonical: PathBuf,
}

impl ObservationKey {
    pub(super) fn for_source(id: &AssetSource, source: &FilesystemSourceIdentity) -> Self {
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
pub(super) struct ResolvedSource {
    pub(super) id: AssetSource,
    pub(super) source_identity: FilesystemSourceIdentity,
}

impl ResolvedSource {
    pub(super) fn observation_key(&self) -> ObservationKey {
        ObservationKey::for_source(&self.id, &self.source_identity)
    }
}

/// The configuration key that declares a source of this kind.
pub(super) fn declaration_key(kind: FilesystemSourceKind) -> &'static str {
    match kind {
        FilesystemSourceKind::Tree => "assets.trees",
        FilesystemSourceKind::File => "assets.files",
    }
}

/// Reader noun for one member of a configured asset tree.
pub(super) const TREE_MEMBER_LABEL: &str = "`assets.trees` member";

/// Source identities taken over by exact declarations in the same owner.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DeclaredFileSources {
    logical: BTreeSet<PathBuf>,
    canonical: BTreeSet<PathBuf>,
}

impl DeclaredFileSources {
    pub(super) fn insert(&mut self, source: PathBuf) {
        let logical = tola_input::lexical_path_identity(&source);
        if let Some(canonical) = FilesystemSourceIdentity::from_path(&logical).canonical_path() {
            self.canonical.insert(canonical.to_path_buf());
        }
        self.logical.insert(logical);
    }

    pub(super) fn owns(&self, logical: &Path, canonical: Option<&Path>) -> bool {
        self.logical.contains(logical)
            || canonical.is_some_and(|canonical| self.canonical.contains(canonical))
    }

    fn publishes(&self, root: &FilesystemSourceIdentity, relative: &Path) -> bool {
        let logical = root.logical_path().join(relative);
        let canonical = root.canonical_path().map(|root| root.join(relative));
        !self.owns(&logical, canonical.as_deref())
    }
}

pub(super) fn read_source(
    config: &ResolvedSiteConfig,
    id: &AssetSource,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<ResolvedSource> {
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
    check_source_boundary(config, id, &source_identity)?;
    Ok(ResolvedSource {
        id: id.clone(),
        source_identity,
    })
}

pub(super) fn check_source_boundary(
    config: &ResolvedSiteConfig,
    id: &AssetSource,
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

pub(super) fn observe_source(
    config: &ResolvedSiteConfig,
    boundary: &tola_typst::SourceBoundary,
    id: &AssetSource,
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
    output::member_path_segments(relative)
        .map(|_| ())
        .map_err(|violation| {
            anyhow::Error::from(AssetOutputError::MemberPath { member, violation })
        })
}

pub(super) fn check_tree_overlap(
    config: &ResolvedSiteConfig,
    sources: &[ResolvedSource],
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
