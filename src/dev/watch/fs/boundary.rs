use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::RecursiveMode;

use super::RebuildScope;
use super::event::{accepted_root_invalidation_kind, is_structural};
use super::producer::{ChangeAdmission, HookWatch, ProducerKind, ProducerSet, WatchRequirement};
use tola_build::filesystem::path_is_within;
use tola_input::lexical_path_identity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverageUncertainty {
    CoverageGap,
    Unreadable,
    MissingRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverageCertainty {
    AcceptedEvents,
    Uncertain(CoverageUncertainty),
}

/// Coverage and freshness of a producer's watched inputs.
#[derive(Debug, Clone)]
pub(super) struct WatchCoverage {
    pub(super) established_epoch: u64,
    pub(super) certainty: CoverageCertainty,
    pub(super) covers_current_revision: bool,
}

impl WatchCoverage {
    pub(super) const fn established_epoch(&self) -> u64 {
        self.established_epoch
    }

    pub(super) const fn certainty(&self) -> CoverageCertainty {
        self.certainty
    }

    pub(super) const fn covers_current_revision(&self) -> bool {
        self.covers_current_revision
    }
}

/// A native directory watch shared by logical subscriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectoryWatch {
    pub(super) path: PathBuf,
    pub(super) mode: RecursiveMode,
}

impl DirectoryWatch {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn recursive_mode(&self) -> RecursiveMode {
        self.mode
    }

    pub(super) fn covers(&self, req: &Self) -> bool {
        if self.mode == RecursiveMode::Recursive {
            req.path.starts_with(&self.path)
        } else {
            self == req
        }
    }
}

/// A complete physical coverage change, committed only after its batch succeeds.
#[derive(Debug)]
pub(super) enum DirectoryWatchUpdate {
    Retain,
    Replace {
        directories: Vec<DirectoryWatch>,
        operations: Vec<notify::PathOp>,
    },
}

impl DirectoryWatchUpdate {
    pub(super) fn take_operations(&mut self) -> Vec<notify::PathOp> {
        match self {
            Self::Retain => Vec::new(),
            Self::Replace { operations, .. } => std::mem::take(operations),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn retains_directories(&self) -> bool {
        matches!(self, Self::Retain)
    }
}

/// Logical subscriptions and the minimum directories needed to observe them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchSelection {
    pub(super) desired: Vec<WatchRequirement>,
    pub(super) boundary: EventBoundary,
    pub(super) directories: Vec<DirectoryWatch>,
    pub(super) directory_producers: BTreeMap<PathBuf, ProducerSet>,
    pub(super) uncertainties: BTreeMap<ProducerKind, CoverageUncertainty>,
}

impl WatchSelection {
    pub(super) fn boundary(&self) -> EventBoundary {
        self.boundary.clone()
    }

    pub(super) fn directory_producers(&self, path: &Path) -> ProducerSet {
        self.directory_producers
            .get(path)
            .copied()
            .unwrap_or_default()
    }
}

/// Logical event matching and the directory entries that maintain its coverage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct EventBoundary {
    /// Sorted by path so event lookup skips unrelated subscriptions.
    pub(super) desired: Arc<[WatchRequirement]>,
    pub(super) producers: ProducerSet,
    pub(super) directories: Arc<[PathBuf]>,
    pub(super) symlink_entries: Arc<[PathBuf]>,
    /// Declared development hook outputs whose current content may account for file events.
    pub(super) hook_outputs: Arc<[tola_input::FilesystemSourceIdentity]>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ClassifiedPath {
    producers: ProducerSet,
    rebuild_scope: RebuildScope,
    hook_output: bool,
}

impl ClassifiedPath {
    pub(super) const fn new(producers: ProducerSet, rebuild_scope: RebuildScope) -> Self {
        Self {
            producers,
            rebuild_scope,
            hook_output: false,
        }
    }

    pub(super) const fn producers(self) -> ProducerSet {
        self.producers
    }

    pub(super) const fn rebuild_scope(self) -> RebuildScope {
        self.rebuild_scope
    }

    pub(super) const fn is_hook_output(self) -> bool {
        self.hook_output
    }

    pub(super) const fn is_empty(self) -> bool {
        self.producers.is_empty()
    }

    pub(super) fn merge(&mut self, req: &WatchRequirement, hook_output: bool) {
        self.producers.insert(req.producers);
        // Output events remain pending until content evidence accounts for the write.
        self.rebuild_scope = if hook_output {
            self.rebuild_scope
                .max(req.rebuild_scope.min(RebuildScope::Paths))
        } else {
            self.rebuild_scope.max(req.rebuild_scope)
        };
        self.hook_output |= hook_output;
    }
}

impl EventBoundary {
    pub(super) fn hook_watches(&self, path: &Path, structural: bool) -> Vec<HookWatch> {
        let path = lexical_path_identity(path);
        self.intersecting_requirements(&path)
            .filter(|req| req.admits_event(&path, structural))
            .flat_map(|req| &req.hook_watches)
            .cloned()
            .collect()
    }

    pub(super) fn hook_outputs(&self) -> Arc<[tola_input::FilesystemSourceIdentity]> {
        Arc::clone(&self.hook_outputs)
    }

    pub(super) fn classify(&self, path: &Path) -> ClassifiedPath {
        self.classify_change(path, notify::EventKind::Any)
    }

    pub(super) fn classify_change(&self, path: &Path, kind: notify::EventKind) -> ClassifiedPath {
        let structural = is_structural(kind);
        let path = lexical_path_identity(path);
        let hook_output = self.is_declared_hook_output(&path);
        self.intersecting_requirements(&path).fold(
            ClassifiedPath::new(ProducerSet::default(), RebuildScope::Paths),
            |mut classified, req| {
                if req.admits_event(&path, structural) {
                    classified.merge(req, hook_output);
                }
                classified
            },
        )
    }

    pub(super) fn is_declared_hook_output(&self, path: &Path) -> bool {
        self.hook_outputs.iter().any(|output| {
            path_is_within(path, output.logical_path())
                || path_is_within(path, output.physical_path())
        })
    }

    pub(super) fn producers(&self) -> ProducerSet {
        self.producers
    }

    /// Only ancestors and descendants can intersect an event; `desired` is path-sorted.
    fn intersecting_requirements<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = &'a WatchRequirement> + 'a {
        let descendants = self
            .desired
            .partition_point(|item| item.path.as_path() < path);
        let ancestors = path.ancestors().skip(1).flat_map(|ancestor| {
            let start = self
                .desired
                .partition_point(|item| item.path.as_path() < ancestor);
            self.desired[start..]
                .iter()
                .take_while(move |item| item.path == ancestor)
        });
        ancestors.chain(
            self.desired[descendants..]
                .iter()
                .take_while(move |item| item.path.starts_with(path)),
        )
    }

    pub(super) fn admits_editor_artifact(&self, path: &Path, structural: bool) -> bool {
        let path = lexical_path_identity(path);
        self.intersecting_requirements(&path)
            .any(|req| match &req.admission {
                ChangeAdmission::AssetTree { .. } | ChangeAdmission::HookRerun { .. } => {
                    req.admits_event(&path, structural)
                }
                ChangeAdmission::Scope | ChangeAdmission::ContentExtension => {
                    path_is_within(&req.path, &path)
                        || (req.matches(&path) && req.admission == ChangeAdmission::Scope)
                }
            })
    }

    /// Directory replacement invalidates coverage; file-only hook output writes leave it intact.
    pub(super) fn invalidated_by(&self, ev: &notify::Event) -> ProducerSet {
        if !accepted_root_invalidation_kind(ev.kind) {
            return ProducerSet::default();
        }
        if file_level_replacement(ev.kind)
            && !ev.paths.is_empty()
            && ev
                .paths
                .iter()
                .all(|path| self.is_declared_hook_output(&lexical_path_identity(path)))
        {
            return ProducerSet::default();
        }
        let touches_root = ev.paths.iter().any(|path| {
            let path = lexical_path_identity(path);
            self.directories
                .iter()
                .any(|dir| path_is_within(dir, &path))
                || self
                    .symlink_entries
                    .iter()
                    .any(|entry| path_is_within(entry, &path))
        });
        if touches_root {
            ProducerSet::ALL
        } else {
            ProducerSet::default()
        }
    }
}

fn file_level_replacement(kind: notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Create(notify::event::CreateKind::File)
            | notify::EventKind::Remove(notify::event::RemoveKind::File)
    )
}

pub(super) fn observation_contains(
    before: &EventBoundary,
    after: &EventBoundary,
    producer: ProducerKind,
) -> bool {
    after
        .desired
        .iter()
        .filter(|req| req.producers.contains(producer))
        .all(|req| {
            req.path.ancestors().any(|ancestor| {
                let first = before
                    .desired
                    .partition_point(|existing| existing.path.as_path() < ancestor);
                before.desired[first..]
                    .iter()
                    .take_while(|existing| existing.path == ancestor)
                    .any(|existing| existing.producers.contains(producer) && existing.covers(req))
            })
        })
}
