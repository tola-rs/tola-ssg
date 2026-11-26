use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::EVENT_DETAIL_CAPACITY;
use super::RebuildScope;
use super::boundary::{ClassifiedPath, EventBoundary, WatchSelection};
use super::dispatch::RebuildPathFilter;
use super::producer::{ProducerKind, ProducerSet};
use super::roots::RootSet;

const COMMITTED_CONTENT_SIZE_LIMIT: u64 = 4 * 1024 * 1024;
const COMMITTED_CONTENT_CAPACITY: usize = 4096;
pub(super) const COMMITTED_CONTENT_READ_BUDGET: u64 = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct EventEpochState {
    pub(super) boundary: EventBoundary,
    pub(super) internal_paths: RebuildPathFilter,
    pub(super) revision: u64,
    pub(super) pending: BTreeMap<PathBuf, PendingEvent>,
    /// File content each committed claim consumed, until a newer claim replaces it.
    pub(super) committed_content: BTreeMap<PathBuf, CommittedFileContent>,
    /// An in-flight or failed attempt keeps every event until its claim commits.
    pub(super) claim_committed: bool,
    pub(super) folded_hooks: Option<FoldedHookEvents>,
    pub(super) uncertainties: BTreeMap<ProducerKind, u64>,
    pub(super) pending_site_rebuild: Option<u64>,
    pub(super) directory_refresh: Option<u64>,
    pub(super) failure: Option<Arc<notify::Error>>,
}

#[derive(Clone, Copy)]
pub(super) struct PendingEvent {
    pub(super) revision: u64,
    pub(super) producers: ProducerSet,
    pub(super) rebuild_scope: RebuildScope,
    pub(super) structural: bool,
    pub(super) represented_by_hook: bool,
}

#[derive(Clone)]
pub(super) struct FoldedHookEvents {
    pub(super) revision: u64,
    pub(super) outputs: Arc<[tola_input::FilesystemSourceIdentity]>,
    pub(super) affected: Vec<bool>,
    pub(super) represented_by_hook: bool,
}

impl FoldedHookEvents {
    pub(super) fn roots(&self) -> impl Iterator<Item = &tola_input::FilesystemSourceIdentity> {
        self.outputs
            .iter()
            .zip(&self.affected)
            .filter_map(|(root, affected)| affected.then_some(root))
    }
}

/// Content digest used to suppress redelivered events whose bytes still match the last commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommittedFileContent {
    len: u64,
    digest: blake3::Hash,
}

pub(super) fn capture_file_content(path: &Path) -> Option<CommittedFileContent> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let len = meta.len();
    if len > COMMITTED_CONTENT_SIZE_LIMIT {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    let copied = std::io::copy(
        &mut std::io::Read::take(&mut file, COMMITTED_CONTENT_SIZE_LIMIT + 1),
        &mut hasher,
    )
    .ok()?;
    (copied == len).then(|| CommittedFileContent {
        len,
        digest: hasher.finalize(),
    })
}

impl EventEpochState {
    pub(super) fn next_revision(&mut self) -> u64 {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("filesystem event epoch overflowed");
        self.revision
    }

    pub(super) fn holds_committed_content(&self, path: &Path, budget: &mut u64) -> bool {
        let Some(committed) = self.committed_content.get(path) else {
            return false;
        };
        if committed.len > *budget {
            return false;
        }
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() && meta.len() == committed.len => {}
            _ => return false,
        }
        let Some(content) = capture_file_content(path) else {
            return false;
        };
        *budget = budget.saturating_sub(content.len);
        content == *committed
    }

    pub(super) fn advance_classified(
        &mut self,
        paths: impl IntoIterator<Item = (PathBuf, ClassifiedPath)>,
    ) {
        let rev = self.next_revision();
        for (path, classified) in paths {
            let producers = classified.producers();
            if !self.pending.contains_key(&path) && self.pending.len() == EVENT_DETAIL_CAPACITY {
                if classified.is_hook_output() && self.fold_hook_path(&path, rev) {
                    continue;
                }
                self.pending_site_rebuild = Some(rev);
                for producer in producers.iter() {
                    self.uncertainties.insert(producer, rev);
                }
                continue;
            }
            self.pending
                .entry(path)
                .and_modify(|pending| {
                    pending.revision = rev;
                    pending.represented_by_hook = false;
                    pending.producers.insert(producers);
                    pending.rebuild_scope = pending.rebuild_scope.max(classified.rebuild_scope());
                })
                .or_insert(PendingEvent {
                    revision: rev,
                    producers,
                    rebuild_scope: classified.rebuild_scope(),
                    structural: false,
                    represented_by_hook: false,
                });
        }
    }

    fn fold_hook_path(&mut self, path: &Path, rev: u64) -> bool {
        let outputs = self.boundary.hook_outputs();
        if self
            .folded_hooks
            .as_ref()
            .is_some_and(|folded| folded.outputs != outputs)
        {
            let mut folded = self
                .folded_hooks
                .take()
                .expect("the previous declaration snapshot exists");
            let mut affected = vec![false; outputs.len()];
            let mut abandoned = false;
            for root in folded.roots() {
                if let Some(idx) = outputs.iter().position(|cur| cur == root) {
                    affected[idx] = true;
                } else {
                    abandoned = true;
                }
            }
            if abandoned && !folded.represented_by_hook {
                self.pending_site_rebuild = Some(
                    self.pending_site_rebuild
                        .map_or(folded.revision, |epoch| epoch.max(folded.revision)),
                );
                for producer in ProducerSet::ALL.iter() {
                    self.uncertainties
                        .entry(producer)
                        .and_modify(|epoch| *epoch = (*epoch).max(folded.revision))
                        .or_insert(folded.revision);
                }
            }
            folded.outputs = Arc::clone(&outputs);
            folded.affected = affected;
            self.folded_hooks = Some(folded);
        }
        let folded = self.folded_hooks.get_or_insert_with(|| FoldedHookEvents {
            revision: rev,
            affected: vec![false; outputs.len()],
            outputs,
            represented_by_hook: false,
        });
        let selected = folded
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(idx, output)| {
                [output.logical_path(), output.physical_path()]
                    .into_iter()
                    .filter(|root| path.starts_with(root))
                    .map(|root| root.components().count())
                    .max()
                    .map(|specificity| (idx, specificity))
            })
            .max_by_key(|(_, specificity)| *specificity);
        if let Some((idx, _)) = selected {
            folded.affected[idx] = true;
            folded.revision = rev;
            folded.represented_by_hook = false;
        }
        selected.is_some()
    }

    pub(super) fn advance_site_rebuild(&mut self, producers: ProducerSet) {
        let rev = self.next_revision();
        self.pending_site_rebuild = Some(rev);
        self.invalidate_directories(producers);
    }

    pub(super) fn invalidate_directories(&mut self, producers: ProducerSet) {
        self.directory_refresh = Some(self.revision);
        for producer in producers.iter() {
            self.uncertainties.insert(producer, self.revision);
        }
    }

    pub(super) fn fail(&mut self, producers: ProducerSet, err: Arc<notify::Error>) {
        let rev = self.next_revision();
        for producer in producers.iter() {
            self.uncertainties.insert(producer, rev);
        }
        self.failure.get_or_insert(err);
    }

    pub(super) fn owed_after(&self, after: Option<u64>) -> (Vec<PathBuf>, RebuildScope) {
        let owed = |rev: u64| after.is_none_or(|after| rev > after);
        let mut rebuild_scope = if self.pending_site_rebuild.is_some_and(owed)
            || self
                .folded_hooks
                .as_ref()
                .is_some_and(|folded| !folded.represented_by_hook && owed(folded.revision))
        {
            RebuildScope::FullSite
        } else {
            RebuildScope::Paths
        };
        let mut paths = Vec::new();
        for (path, observed) in &self.pending {
            if observed.represented_by_hook || !owed(observed.revision) {
                continue;
            }
            paths.push(path.clone());
            rebuild_scope = rebuild_scope.max(observed.rebuild_scope);
        }
        (paths, rebuild_scope)
    }
}

#[derive(Clone, Default)]
pub(super) struct EventEpoch(Arc<Mutex<EventEpochState>>);

impl EventEpoch {
    pub(super) fn for_root(root: &Path) -> Self {
        let epoch = Self::default();
        epoch.lock().internal_paths = RebuildPathFilter::internal(root);
        epoch
    }

    /// Recover poisoned state so a session keeps serving its last consistent state.
    pub(super) fn lock(&self) -> MutexGuard<'_, EventEpochState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn requires_directory_refresh(&self) -> bool {
        self.lock().directory_refresh.is_some()
    }

    pub(super) fn revision(&self) -> u64 {
        self.lock().revision
    }

    pub(super) fn claim_candidate(&self) -> EventClaim {
        let epoch = self.lock();
        let (paths, rebuild_scope) = epoch.owed_after(None);
        EventClaim {
            modified_paths: epoch
                .pending
                .iter()
                .filter(|(_, ev)| !ev.structural)
                .map(|(path, _)| path.clone())
                .collect(),
            revision: epoch.revision,
            paths,
            rebuild_scope,
            committed_content: Vec::new(),
        }
    }

    pub(super) fn changes_after(&self, rev: u64) -> super::FileChangeBatch {
        let (paths, rebuild_scope) = self.lock().owed_after(Some(rev));
        super::FileChangeBatch {
            paths,
            rebuild_scope,
        }
    }

    pub(super) fn is_uncertain_after(&self, producer: ProducerKind, rev: u64) -> bool {
        self.lock()
            .uncertainties
            .get(&producer)
            .is_some_and(|uncertainty| *uncertainty > rev)
    }

    pub(super) fn guard_relevant_confirmed(
        &self,
        expected: u64,
        mut relevant: impl FnMut(&Path) -> bool,
        confirmed: &ConfirmedHookPaths,
    ) -> Result<Option<EventEpochStateGuard<'_>>, Arc<notify::Error>> {
        let guard = self.lock();
        if let Some(err) = guard.failure.as_ref() {
            return Err(Arc::clone(err));
        }
        let confirmed_hook_paths = guard
            .pending
            .iter()
            .filter(|(path, observed)| observed.revision > expected && confirmed.confirms(path))
            .map(|(path, observed)| (path.clone(), observed.revision))
            .collect::<BTreeMap<_, _>>();
        let confirmed_folded_revision = confirmed.folded_revision.filter(|rev| {
            guard
                .folded_hooks
                .as_ref()
                .is_some_and(|folded| folded.revision == *rev)
        });
        let superseded = guard.directory_refresh.is_some()
            || guard
                .pending_site_rebuild
                .is_some_and(|observed| observed > expected)
            || guard.folded_hooks.as_ref().is_some_and(|folded| {
                folded.revision > expected
                    && !folded.represented_by_hook
                    && confirmed_folded_revision != Some(folded.revision)
            })
            || guard.pending.iter().any(|(path, observed)| {
                observed.revision > expected
                    && !observed.represented_by_hook
                    && !confirmed.confirms(path)
                    && (observed.rebuild_scope > RebuildScope::Paths || relevant(path))
            });
        Ok((!superseded).then_some(EventEpochStateGuard {
            guard,
            confirmed_hook_paths,
            confirmed_folded_revision,
        }))
    }
}

/// Pending filesystem changes for one build candidate; dropping a claim consumes nothing.
#[derive(Debug)]
pub(crate) struct EventClaim {
    pub(super) revision: u64,
    pub(super) paths: Vec<PathBuf>,
    pub(super) committed_content: Vec<(PathBuf, CommittedFileContent)>,
    pub(super) modified_paths: BTreeSet<PathBuf>,
    pub(super) rebuild_scope: RebuildScope,
}

impl EventClaim {
    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    /// Narrow modified ancestors to nested pending paths while keeping structural ancestors.
    pub(crate) fn merge_into(&self, paths: &mut Vec<PathBuf>) -> RebuildScope {
        for path in &self.paths {
            if !paths.contains(path) {
                if paths.len() == EVENT_DETAIL_CAPACITY {
                    paths.clear();
                    return RebuildScope::FullSite;
                }
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths.dedup();
        let mut entries = std::mem::take(paths).into_iter().peekable();
        while let Some(path) = entries.next() {
            if self.modified_paths.contains(&path)
                && entries.peek().is_some_and(|next| next.starts_with(&path))
            {
                continue;
            }
            paths.push(path);
        }
        self.rebuild_scope
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn rebuild_scope(&self) -> RebuildScope {
        self.rebuild_scope
    }
}

#[derive(Default)]
pub(super) struct ConfirmedHookPaths {
    pub(super) paths: BTreeSet<PathBuf>,
    pub(super) folded_revision: Option<u64>,
}

impl ConfirmedHookPaths {
    pub(super) fn confirms(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }
}

pub(super) struct EventEpochStateGuard<'a> {
    guard: MutexGuard<'a, EventEpochState>,
    confirmed_hook_paths: BTreeMap<PathBuf, u64>,
    confirmed_folded_revision: Option<u64>,
}

impl EventEpochStateGuard<'_> {
    fn commit(mut self, claim: EventClaim) {
        for (path, content) in claim.committed_content {
            if self.guard.committed_content.len() == COMMITTED_CONTENT_CAPACITY
                && !self.guard.committed_content.contains_key(&path)
            {
                self.guard.committed_content.clear();
            }
            self.guard.committed_content.insert(path, content);
        }
        self.guard.claim_committed = true;
        self.guard.pending.retain(|path, observed| {
            observed.revision > claim.revision
                && self.confirmed_hook_paths.get(path).copied() != Some(observed.revision)
        });
        if self.guard.folded_hooks.as_ref().is_some_and(|folded| {
            folded.revision <= claim.revision
                || self.confirmed_folded_revision == Some(folded.revision)
        }) {
            self.guard.folded_hooks = None;
        }
        if self
            .guard
            .pending_site_rebuild
            .is_some_and(|observed| observed <= claim.revision)
        {
            self.guard.pending_site_rebuild = None;
        }
    }
}

/// Linearizes a revision replacement with accepted input events and subscriptions.
pub(crate) struct EventGuard<'a> {
    pub(super) epoch: EventEpochStateGuard<'a>,
    pub(super) roots: &'a mut RootSet,
}

impl EventGuard<'_> {
    pub(crate) fn commit(mut self, claim: EventClaim, sel: WatchSelection) {
        self.roots.commit_logical(sel, self.epoch.guard.revision);
        self.roots.confirm_current_observation();
        let boundary = self.roots.boundary();
        self.epoch
            .guard
            .pending
            .retain(|path, _| !boundary.classify(path).is_empty());
        self.epoch.guard.boundary = boundary;
        self.epoch.commit(claim);
    }
}
