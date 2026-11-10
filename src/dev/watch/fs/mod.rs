//! Filesystem changes for development rebuilds.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tola_build::config::ResolvedSiteConfig;

mod debouncer;
mod event;
mod observer;
mod path;
mod roots;

use debouncer::Debouncer;
use observer::DirectoryObserver;
use path::is_editor_temp;
pub(crate) use roots::WatchSelection;
use roots::{ClassifiedPath, EventBoundary, RootSet};
pub(crate) use roots::{ProducerKind, ProducerSet, WatchRequirements};

/// Failure to start or maintain filesystem watching.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WatchError {
    #[error("failed to create filesystem watcher: {0}")]
    Create(#[source] notify::Error),
    #[error("filesystem watcher reported an error: {0}")]
    Event(#[source] Arc<notify::Error>),
    #[error("filesystem watcher stopped unexpectedly")]
    Stopped,
    #[error("failed to observe generated inputs: {0}")]
    HookInputs(#[source] anyhow::Error),
    #[error("could not watch `rerun-on` paths: {0}")]
    RerunOn(#[source] anyhow::Error),
    #[error("could not watch a configured path")]
    Attach {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

/// Rebuild scope required by accepted filesystem changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum RebuildScope {
    /// Rebuild only producers affected by the accepted paths.
    #[default]
    Paths,
    /// Recompile every Typst unit while retaining non-Typst inventories.
    FullTypst,
    /// Rebuild every producer from a fresh site observation.
    FullSite,
}

/// A debounced batch of accepted filesystem changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileChangeBatch {
    paths: Vec<PathBuf>,
    rebuild_scope: RebuildScope,
}

impl FileChangeBatch {
    pub(crate) fn into_parts(self) -> (Vec<PathBuf>, RebuildScope) {
        (self.paths, self.rebuild_scope)
    }
}

#[derive(Default)]
struct EventEpochState {
    boundary: EventBoundary,
    revision: u64,
    pending: BTreeMap<PathBuf, PendingEvent>,
    /// File content each committed claim consumed, until a newer claim replaces it.
    committed_content: BTreeMap<PathBuf, CommittedFileContent>,
    /// Whether the newest claim committed; an in-flight or failed attempt keeps every event.
    claim_committed: bool,
    folded_hooks: Option<FoldedHookEvents>,
    uncertainties: BTreeMap<ProducerKind, u64>,
    pending_site_rebuild: Option<u64>,
    // Coverage needs an update that started at or after this event epoch.
    directory_refresh: Option<u64>,
    failure: Option<Arc<notify::Error>>,
}

#[derive(Clone, Copy)]
struct PendingEvent {
    revision: u64,
    producers: ProducerSet,
    rebuild_scope: RebuildScope,
    structural: bool,
    represented_by_hook: bool,
}

/// Spill identifies configured output roots, never individual descendant paths. The bit vector
/// is fixed by the hook-output declarations it was built from; an ancestor cannot be
/// attributed to its output.
#[derive(Clone)]
struct FoldedHookEvents {
    revision: u64,
    outputs: Arc<[tola_input::FilesystemSourceIdentity]>,
    affected: Vec<bool>,
    represented_by_hook: bool,
}

impl FoldedHookEvents {
    fn roots(&self) -> impl Iterator<Item = &tola_input::FilesystemSourceIdentity> {
        self.outputs
            .iter()
            .zip(&self.affected)
            .filter_map(|(root, affected)| affected.then_some(root))
    }
}

/// One file's content as a committed claim observed it.
///
/// A redelivered event whose path still holds exactly these bytes describes no change the
/// installed revision does not already reflect, so it owes no rebuild. Timestamps are not part
/// of the comparison: rewriting the same bytes cannot change the build's output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommittedFileContent {
    len: u64,
    digest: blake3::Hash,
}

/// The largest file whose redelivery may be settled by reading its bytes.
///
/// Hashing a larger file under the event epoch costs more than re-validating its event, so a
/// larger file's events always stay.
const COMMITTED_CONTENT_SIZE_LIMIT: u64 = 4 * 1024 * 1024;

/// How many paths' committed content one session retains.
///
/// A session that commits more paths drops the whole map; a redelivered event is then rebuilt
/// instead of being settled by content, which is the behavior without the map.
const COMMITTED_CONTENT_CAPACITY: usize = 4096;

/// Bytes one event may read to settle redeliveries.
///
/// Beyond it, the remaining paths keep their events and rebuild, as an unprovable path does; a
/// checkout touching many large files must not read them all while holding the event epoch.
const COMMITTED_CONTENT_READ_BUDGET: u64 = 16 * 1024 * 1024;

/// Capture one path's content for redelivery comparison.
///
/// Only a regular file whose bytes are all read yields a capture; a directory, a symlink, an
/// oversized or unreadable file, or one that changes while being read stays without one, and a
/// path without a capture never suppresses an event.
fn capture_file_content(path: &Path) -> Option<CommittedFileContent> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let len = metadata.len();
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
    if copied != len {
        return None;
    }
    Some(CommittedFileContent {
        len,
        digest: hasher.finalize(),
    })
}

impl EventEpochState {
    fn next_revision(&mut self) -> u64 {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("filesystem event epoch overflowed");
        self.revision
    }

    /// Whether `path` still holds exactly the content a committed claim consumed, spending at
    /// most `budget` bytes of this event's reads.
    ///
    /// A path without a committed capture — never claimed, a directory, a symlink, an oversized
    /// file, or one that cannot be read — never settles this way.
    fn holds_committed_content(&self, path: &Path, budget: &mut u64) -> bool {
        let Some(committed) = self.committed_content.get(path) else {
            return false;
        };
        if committed.len > *budget {
            return false;
        }
        // Only a same-length path can match; skip the read when the length already decides.
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.len() == committed.len => {}
            _ => return false,
        }
        let Some(content) = capture_file_content(path) else {
            return false;
        };
        *budget = budget.saturating_sub(content.len);
        content == *committed
    }

    fn advance_classified(&mut self, paths: impl IntoIterator<Item = (PathBuf, ClassifiedPath)>) {
        let revision = self.next_revision();
        for (path, classified) in paths {
            let producers = classified.producers();
            if !self.pending.contains_key(&path) && self.pending.len() == EVENT_DETAIL_CAPACITY {
                if classified.is_hook_output() && self.fold_hook_path(&path, revision) {
                    continue;
                }
                self.pending_site_rebuild = Some(revision);
                for producer in producers.iter() {
                    self.uncertainties.insert(producer, revision);
                }
                continue;
            }
            self.pending
                .entry(path)
                .and_modify(|pending| {
                    pending.revision = revision;
                    pending.represented_by_hook = false;
                    pending.producers.insert(producers);
                    pending.rebuild_scope = pending.rebuild_scope.max(classified.rebuild_scope());
                })
                .or_insert(PendingEvent {
                    revision,
                    producers,
                    rebuild_scope: classified.rebuild_scope(),
                    structural: false,
                    represented_by_hook: false,
                });
        }
    }

    fn fold_hook_path(&mut self, path: &Path, revision: u64) -> bool {
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
                if let Some(index) = outputs.iter().position(|current| current == root) {
                    affected[index] = true;
                } else {
                    abandoned = true;
                }
            }
            if abandoned && !folded.represented_by_hook {
                // Abandoned declarations stay owed at their own epoch, not at the new hook's write.
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
            revision,
            affected: vec![false; outputs.len()],
            outputs,
            represented_by_hook: false,
        });
        // Recovery may watch both a published parent and an attempted child; child evidence
        // accounts for its own descendants without claiming that the parent stayed unchanged.
        let selected = folded
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(index, output)| {
                [output.logical_path(), output.physical_path()]
                    .into_iter()
                    .filter(|root| path.starts_with(root))
                    .map(|root| root.components().count())
                    .max()
                    .map(|specificity| (index, specificity))
            })
            .max_by_key(|(_, specificity)| *specificity);
        if let Some((index, _)) = selected {
            folded.affected[index] = true;
            folded.revision = revision;
            folded.represented_by_hook = false;
        }
        selected.is_some()
    }

    fn advance_site_rebuild(&mut self, producers: ProducerSet) {
        let revision = self.next_revision();
        self.pending_site_rebuild = Some(revision);
        self.invalidate_directories(producers);
    }

    fn invalidate_directories(&mut self, producers: ProducerSet) {
        self.directory_refresh = Some(self.revision);
        for producer in producers.iter() {
            self.uncertainties.insert(producer, self.revision);
        }
    }

    fn fail(&mut self, producers: ProducerSet, error: Arc<notify::Error>) {
        let revision = self.next_revision();
        for producer in producers.iter() {
            self.uncertainties.insert(producer, revision);
        }
        self.failure.get_or_insert(error);
    }

    /// Paths a candidate still owes and the scope their pending events require.
    ///
    /// `after: None` claims every unacknowledged path; hook-represented paths never owe a
    /// rebuild, and an unacknowledged site rebuild widens the scope to `FullSite`.
    fn owed_after(&self, after: Option<u64>) -> (Vec<PathBuf>, RebuildScope) {
        let owed = |revision: u64| after.is_none_or(|after| revision > after);
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
struct EventEpoch(Arc<Mutex<EventEpochState>>);

impl EventEpoch {
    /// Poisoned state is recovered rather than propagated, matching the hook mailbox: a session
    /// keeps serving its last consistent state.
    fn lock(&self) -> MutexGuard<'_, EventEpochState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn requires_directory_refresh(&self) -> bool {
        self.lock().directory_refresh.is_some()
    }

    fn revision(&self) -> u64 {
        self.lock().revision
    }

    fn claim_candidate(&self) -> EventClaim {
        let epoch = self.lock();
        let (paths, rebuild_scope) = epoch.owed_after(None);
        EventClaim {
            modified_paths: epoch
                .pending
                .iter()
                .filter(|(_, event)| !event.structural)
                .map(|(path, _)| path.clone())
                .collect(),
            revision: epoch.revision,
            paths,
            rebuild_scope,
            committed_content: Vec::new(),
        }
    }

    fn changes_after(&self, revision: u64) -> FileChangeBatch {
        let (paths, rebuild_scope) = self.lock().owed_after(Some(revision));
        FileChangeBatch {
            paths,
            rebuild_scope,
        }
    }

    fn is_uncertain_after(&self, producer: ProducerKind, revision: u64) -> bool {
        self.lock()
            .uncertainties
            .get(&producer)
            .is_some_and(|uncertainty| *uncertainty > revision)
    }

    fn guard_relevant_confirmed(
        &self,
        expected: u64,
        mut relevant: impl FnMut(&Path) -> bool,
        confirmed: &ConfirmedHookPaths,
    ) -> Result<Option<EventEpochStateGuard<'_>>, Arc<notify::Error>> {
        let guard = self.lock();
        if let Some(error) = guard.failure.as_ref() {
            return Err(Arc::clone(error));
        }
        let confirmed_hook_paths = guard
            .pending
            .iter()
            .filter(|(path, observed)| observed.revision > expected && confirmed.confirms(path))
            .map(|(path, observed)| (path.clone(), observed.revision))
            .collect::<BTreeMap<_, _>>();
        let confirmed_folded_revision = confirmed.folded_revision.filter(|revision| {
            guard
                .folded_hooks
                .as_ref()
                .is_some_and(|folded| folded.revision == *revision)
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

/// Pending filesystem changes for one build candidate; dropping a claim consumes nothing, so
/// paths and full-rescan markers stay pending until a published revision acknowledges them.
#[derive(Debug)]
pub(crate) struct EventClaim {
    revision: u64,
    paths: Vec<PathBuf>,
    /// File contents captured when the claim began; only a commit records them.
    committed_content: Vec<(PathBuf, CommittedFileContent)>,
    modified_paths: std::collections::BTreeSet<PathBuf>,
    rebuild_scope: RebuildScope,
}

impl EventClaim {
    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    /// A modified ancestor whose next pending path lies inside it narrows to that path;
    /// structural ancestors stay, so neither end of a rename is dropped.
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

    pub(crate) const fn rebuild_scope(&self) -> RebuildScope {
        self.rebuild_scope
    }
}

#[derive(Debug)]
enum WatchMessage {
    Event {
        event: notify::Event,
        revision: u64,
        classified_paths: Vec<(PathBuf, ClassifiedPath)>,
        producers: ProducerSet,
        invalidated: ProducerSet,
    },
    Error {
        producers: ProducerSet,
        error: Arc<notify::Error>,
    },
}

impl WatchMessage {
    fn producers(&self) -> ProducerSet {
        match self {
            Self::Event { producers, .. } | Self::Error { producers, .. } => *producers,
        }
    }
}

// Every retained event-detail container shares this bound; spill stays owed by its epoch.
pub(crate) const EVENT_DETAIL_CAPACITY: usize = 256;

trait WatchMessageSink {
    /// Enqueue a message; return producers when overflow is coalesced.
    fn send_message(&self, message: WatchMessage) -> Option<ProducerSet>;

    fn mark_overflow_ready(&self, _revision: u64) {}
}

#[derive(Debug)]
struct EventOverflow {
    producers: ProducerSet,
    revision: u64,
    failure: Option<Arc<notify::Error>>,
}

impl EventOverflow {
    fn include(&mut self, message: &WatchMessage) {
        match message {
            WatchMessage::Event {
                producers,
                revision,
                ..
            } => {
                self.producers.insert(*producers);
                self.revision = self.revision.max(*revision);
            }
            WatchMessage::Error { producers, error } => {
                self.producers.insert(*producers);
                if self.failure.is_none() {
                    self.failure = Some(Arc::clone(error));
                }
            }
        }
    }

    fn into_message(self) -> WatchMessage {
        if let Some(error) = self.failure {
            WatchMessage::Error {
                producers: self.producers,
                error,
            }
        } else {
            WatchMessage::Event {
                revision: self.revision,
                event: notify::Event::new(notify::EventKind::Other)
                    .set_flag(notify::event::Flag::Rescan),
                classified_paths: Vec::new(),
                producers: self.producers,
                invalidated: self.producers,
            }
        }
    }
}

#[derive(Default)]
struct WatchEventQueueState {
    messages: VecDeque<WatchMessage>,
    queued_paths: usize,
    overflow: Option<EventOverflow>,
    overflow_ready: bool,
    closed: bool,
}

#[derive(Clone)]
struct WatchEventQueue {
    state: Arc<Mutex<WatchEventQueueState>>,
    wake: Arc<tokio::sync::Notify>,
}

struct WatchEventReceiver {
    queue: WatchEventQueue,
}

impl WatchEventQueue {
    fn lock(&self) -> MutexGuard<'_, WatchEventQueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn channel() -> (Self, WatchEventReceiver) {
        let queue = Self {
            state: Arc::new(Mutex::new(WatchEventQueueState::default())),
            wake: Arc::new(tokio::sync::Notify::new()),
        };
        (
            queue.clone(),
            WatchEventReceiver {
                queue: queue.clone(),
            },
        )
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        if state.overflow.is_some() {
            state.overflow_ready = true;
        }
        drop(state);
        self.wake.notify_waiters();
    }

    fn mark_overflow_ready_inner(&self, revision: u64) {
        let mut state = self.lock();
        if let Some(overflow) = state.overflow.as_mut() {
            overflow.revision = revision;
            state.overflow_ready = true;
        }
        drop(state);
        self.wake.notify_waiters();
    }
}

impl WatchMessageSink for WatchEventQueue {
    fn send_message(&self, message: WatchMessage) -> Option<ProducerSet> {
        let mut state = self.lock();
        if state.closed {
            return None;
        }
        if let Some(overflow) = state.overflow.as_mut() {
            let producers = message.producers();
            overflow.include(&message);
            drop(state);
            self.wake.notify_one();
            return Some(producers);
        }
        let path_count = match &message {
            WatchMessage::Event { event, .. } => event.paths.len(),
            WatchMessage::Error { .. } => 0,
        };
        if state.messages.len() < EVENT_DETAIL_CAPACITY
            && state.queued_paths + path_count <= EVENT_DETAIL_CAPACITY
        {
            state.queued_paths += path_count;
            state.messages.push_back(message);
            drop(state);
            self.wake.notify_one();
            return None;
        }

        let mut overflow = EventOverflow {
            producers: ProducerSet::default(),
            revision: 0,
            failure: None,
        };
        for pending in state.messages.drain(..) {
            overflow.include(&pending);
        }
        state.queued_paths = 0;
        overflow.include(&message);
        let producers = overflow.producers;
        state.overflow = Some(overflow);
        state.overflow_ready = false;
        drop(state);
        self.wake.notify_one();
        Some(producers)
    }

    fn mark_overflow_ready(&self, revision: u64) {
        self.mark_overflow_ready_inner(revision);
    }
}

impl WatchEventReceiver {
    async fn recv(&mut self) -> Option<WatchMessage> {
        loop {
            // Created while the queue state is observed, so any later signal is
            // either already visible here or counted from this future's creation.
            let notified = {
                let mut state = self.queue.lock();
                if state.overflow_ready {
                    state.overflow_ready = false;
                    return state.overflow.take().map(EventOverflow::into_message);
                }
                if let Some(message) = state.messages.pop_front() {
                    if let WatchMessage::Event { event, .. } = &message {
                        state.queued_paths -= event.paths.len();
                    }
                    return Some(message);
                }
                if state.closed {
                    return None;
                }
                self.queue.wake.notified()
            };
            notified.await;
        }
    }
}

/// Input subscriptions and directory watching for one development session.
pub(crate) struct FileChangeSource {
    event_tx: WatchEventQueue,
    events: WatchEventReceiver,
    event_epoch: EventEpoch,
    claimed_revision: u64,
    observer: Option<DirectoryObserver>,
    roots: RootSet,
    debouncer: Debouncer,
    hook_candidate: Option<QuarantinedHookPaths>,
    // Failed candidates can leave current generated inputs without installing a revision.
    source_hooks: tola_build::hooks::SourceHookOutputs,
    // Files left by failed scripts cannot justify reusing hook outputs.
    failed_hook_outputs: Option<Arc<tola_build::hooks::FailedHookOutputs>>,
    hook_path_check: Option<RunningHookPathCheck>,
}

/// Paths one observation of declared hook outputs represents.
///
/// Content decides: the observation read each path, so the event that reported a write to it is
/// the write that was read. Order does not, because a hook that rewrites its own declared output
/// produces several events for one path, and a session that counted a later one as a change no
/// output accounts for would rebuild for its own write.
#[derive(Default)]
struct ConfirmedHookPaths {
    paths: std::collections::BTreeSet<PathBuf>,
    folded_revision: Option<u64>,
}

impl ConfirmedHookPaths {
    fn confirms(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }
}

struct RunningHookPathCheck {
    canceller: tola_build::cancellation::BuildCanceller,
    worker: tokio::task::JoinHandle<anyhow::Result<ConfirmedHookPaths>>,
}

struct QuarantinedHookPaths {
    source_identities: Vec<tola_input::FilesystemSourceIdentity>,
    paths: std::collections::BTreeSet<PathBuf>,
    rebuild_scope: RebuildScope,
    overflowed: bool,
}

impl QuarantinedHookPaths {
    /// The batch this candidate implies, or none when it observed nothing.
    fn into_batch(self) -> Option<FileChangeBatch> {
        (!self.paths.is_empty() || self.overflowed).then(|| FileChangeBatch {
            paths: self.paths.into_iter().collect(),
            rebuild_scope: if self.overflowed {
                RebuildScope::FullSite
            } else {
                self.rebuild_scope
            },
        })
    }
}

struct EventEpochStateGuard<'a> {
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
    epoch: EventEpochStateGuard<'a>,
    roots: &'a mut RootSet,
}

impl EventGuard<'_> {
    pub(crate) fn commit(mut self, claim: EventClaim, selection: WatchSelection) {
        self.roots
            .commit_logical(selection, self.epoch.guard.revision);
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

impl FileChangeSource {
    pub(crate) async fn start(
        config: &ResolvedSiteConfig,
        dynamic: &WatchRequirements,
        source_hooks: &tola_build::hooks::SourceHookOutputs,
        log_file: Option<&Path>,
    ) -> Result<Self, WatchError> {
        let (event_tx, events) = WatchEventQueue::channel();
        let event_epoch = EventEpoch::default();
        let roots = RootSet::from_config(config, dynamic).map_err(WatchError::RerunOn)?;
        let callback_epoch = event_epoch.clone();
        let callback_events = event_tx.clone();
        let log_file = log_file.map(tola_input::normalize_existing_prefix);
        let observer = DirectoryObserver::start(move || {
            notify::recommended_watcher(move |event| {
                forward_watch_result(
                    event,
                    &callback_epoch,
                    &callback_events,
                    log_file.as_deref(),
                );
            })
        })
        .await
        .map_err(WatchError::Create)?;
        let mut source = Self {
            event_tx,
            events,
            event_epoch,
            claimed_revision: 0,
            observer: Some(observer),
            roots,
            debouncer: Debouncer::new(),
            hook_candidate: None,
            source_hooks: source_hooks.clone(),
            failed_hook_outputs: None,
            hook_path_check: None,
        };
        if let Err(error) = source
            .install_selection(source.roots.initial_selection(), false)
            .await
        {
            source.shutdown().await?;
            return Err(error);
        }
        Ok(source)
    }

    /// Watch both current and candidate inputs; return the candidate's subscriptions.
    pub(crate) async fn stage_config(
        &mut self,
        config: &ResolvedSiteConfig,
        dynamic: &WatchRequirements,
    ) -> Result<WatchSelection, WatchError> {
        let (observing, subscriptions, retain) = self
            .roots
            .staged_selection(config, dynamic)
            .map_err(WatchError::RerunOn)?;
        self.install_selection(observing, retain).await?;
        Ok(subscriptions)
    }

    /// A failed read needs rechecking when its input was outside prior observation.
    pub(crate) async fn reconfigure_configs<'a>(
        &mut self,
        configs: impl IntoIterator<Item = &'a ResolvedSiteConfig>,
        dynamic: &WatchRequirements,
    ) -> Result<bool, WatchError> {
        let (selection, retain) = self
            .roots
            .configs_selection(configs, dynamic)
            .map_err(WatchError::RerunOn)?;
        self.install_selection(selection, retain).await
    }

    pub(crate) async fn refresh_pending(&mut self) -> Result<(), WatchError> {
        if self.event_epoch.requires_directory_refresh() {
            self.install_selection(self.roots.current_selection(), false)
                .await?;
        }
        Ok(())
    }

    pub(crate) fn begin_candidate(&mut self, config: &ResolvedSiteConfig) -> EventClaim {
        self.hook_candidate = Some(QuarantinedHookPaths {
            source_identities: hook_output_source_identities(config),
            paths: std::collections::BTreeSet::new(),
            rebuild_scope: RebuildScope::Paths,
            overflowed: false,
        });
        let rebuild_paths = RebuildPathFilter::for_config(config);
        // A new claim supersedes the committed one until it commits in turn.
        self.event_epoch.lock().claim_committed = false;
        let mut claim = self.event_epoch.claim_candidate();
        self.claimed_revision = claim.revision;
        claim.paths.retain(|path| rebuild_paths.accepts(path));
        claim.committed_content = claim
            .paths
            .iter()
            .filter_map(|path| capture_file_content(path).map(|content| (path.clone(), content)))
            .collect();
        claim
    }

    pub(crate) fn update_hook_candidate_claims(&mut self, config: &ResolvedSiteConfig) {
        if let Some(candidate) = self.hook_candidate.as_mut() {
            candidate.source_identities = hook_output_source_identities(config);
        }
    }

    /// Add the module outputs a prepared hook watch declares, before any hook starts.
    pub(crate) fn extend_hook_candidate_claims(&mut self, inputs: &tola_module::HookWatchInputs) {
        if let Some(candidate) = self.hook_candidate.as_mut() {
            candidate
                .source_identities
                .extend(generated_source_identities(inputs));
        }
    }

    pub(crate) fn reject_hook_candidate(&mut self) -> Option<FileChangeBatch> {
        self.hook_candidate.take()?.into_batch()
    }

    pub(crate) fn complete_hook_candidate(
        &mut self,
        source_hooks: &tola_build::hooks::SourceHookOutputs,
    ) {
        self.hook_candidate = None;
        self.source_hooks = source_hooks.clone();
        self.failed_hook_outputs = None;
    }

    pub(crate) async fn settle_failed_hook_candidate(
        &mut self,
        source_hooks: &tola_build::hooks::SourceHookOutputs,
        failed_hook_outputs: Option<&tola_build::hooks::FailedHookOutputs>,
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<Option<FileChangeBatch>, WatchError> {
        let paths = self.event_epoch.claim_candidate().paths;
        let failed_hook_outputs = failed_hook_outputs.cloned().map(Arc::new);
        let confirmed = self
            .observe_hook_paths(
                paths,
                source_hooks.outputs(),
                failed_hook_outputs.clone(),
                cancellation.clone(),
            )
            .await?;
        let mut epoch = self.event_epoch.lock();
        for (path, pending) in &mut epoch.pending {
            if confirmed.confirms(path) {
                pending.represented_by_hook = true;
            }
        }
        let folded_confirmed = epoch.folded_hooks.as_mut().is_some_and(|folded| {
            if confirmed.folded_revision == Some(folded.revision) {
                folded.represented_by_hook = true;
                true
            } else {
                false
            }
        });
        let retry = self.hook_candidate.take().and_then(|mut candidate| {
            candidate.paths.retain(|path| !confirmed.confirms(path));
            if folded_confirmed
                && epoch
                    .pending_site_rebuild
                    .is_none_or(|revision| revision <= self.claimed_revision)
            {
                candidate.overflowed = false;
            }
            candidate.into_batch()
        });
        drop(epoch);
        self.source_hooks = source_hooks.clone();
        self.failed_hook_outputs = failed_hook_outputs;
        Ok(retry)
    }

    pub(crate) fn event_epoch(&self) -> u64 {
        self.event_epoch.revision()
    }

    pub(crate) fn accepts_events_for(
        &self,
        producers: impl IntoIterator<Item = ProducerKind>,
    ) -> bool {
        !self.event_epoch.requires_directory_refresh()
            && producers.into_iter().all(|producer| {
                self.roots.coverage(producer).is_some_and(|coverage| {
                    coverage.certainty() == roots::CoverageCertainty::AcceptedEvents
                        && coverage.covers_current_revision()
                        && !self
                            .event_epoch
                            .is_uncertain_after(producer, coverage.established_epoch())
                })
            })
    }

    pub(crate) fn rebuild_changes_after(
        &self,
        revision: u64,
        config: &ResolvedSiteConfig,
    ) -> FileChangeBatch {
        let rebuild_paths = RebuildPathFilter::for_config(config);
        let mut changes = self.event_epoch.changes_after(revision);
        changes.paths.retain(|path| rebuild_paths.accepts(path));
        changes
    }

    pub(crate) async fn guard_epoch(
        &mut self,
        expected: u64,
        config: &ResolvedSiteConfig,
        hook_outputs: &[tola_build::hooks::HookOutputEvidence],
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<Option<EventGuard<'_>>, WatchError> {
        if self.event_epoch.requires_directory_refresh() {
            return Ok(None);
        }
        let paths = self.event_epoch.changes_after(expected).paths;
        let rebuild_paths = RebuildPathFilter::for_config(config);
        let ignored = paths
            .iter()
            .filter(|path| !rebuild_paths.accepts(path))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let confirmed = self
            .observe_hook_paths(paths, hook_outputs, None, cancellation.clone())
            .await?;
        let Self {
            event_epoch, roots, ..
        } = self;
        event_epoch
            // A newly accepted path is conservatively relevant until it can be classified.
            .guard_relevant_confirmed(expected, |path| !ignored.contains(path), &confirmed)
            .map(|guard| guard.map(|epoch| EventGuard { epoch, roots }))
            .map_err(WatchError::Event)
    }

    pub(crate) async fn next(
        &mut self,
        config: &ResolvedSiteConfig,
    ) -> Result<FileChangeBatch, WatchError> {
        let rebuild_paths = RebuildPathFilter::for_config(config);
        loop {
            let now = std::time::Instant::now();
            let remaining = self.debouncer.remaining(now);
            // An already-readable event queue must not postpone an expired batch.
            if remaining.is_some_and(|remaining| remaining.is_zero()) {
                let newer = self.event_epoch.changes_after(self.claimed_revision);
                let rebuild_scope = newer.rebuild_scope;
                if let Some(mut batch) =
                    take_rebuild_batch(&mut self.debouncer, &rebuild_paths, rebuild_scope, now)
                {
                    batch.paths = newer
                        .paths
                        .into_iter()
                        .filter(|path| {
                            rebuild_paths.accepts(path) && !self.roots.classify(path).is_empty()
                        })
                        .collect();
                    if self.hook_candidate.is_none() {
                        // Idle receives are cancelled only for shutdown. The
                        // source retains and joins any unfinished observation.
                        if self.suppress_observed_hook_changes(&mut batch).await? {
                            continue;
                        }
                    }
                    if !batch.paths.is_empty() || batch.rebuild_scope > RebuildScope::Paths {
                        return Ok(batch);
                    }
                }
                continue;
            }
            tokio::select! {
                event = self.events.recv() => {
                    let Some(event) = event else {
                        return Err(WatchError::Stopped);
                    };
                    let (mut event, invalidated, revision) = match event {
                        WatchMessage::Event { event, invalidated, revision, .. } => (event, invalidated, revision),
                        WatchMessage::Error { error, .. } => return Err(WatchError::Event(error)),
                    };
                    if revision <= self.claimed_revision {
                        continue;
                    }
                    let rescan = event.need_rescan();
                    if rescan {
                        let mut batch = self.event_epoch.changes_after(self.claimed_revision);
                        event.paths = std::mem::take(&mut batch.paths);
                        if self.hook_candidate.is_some() {
                            self.quarantine_hook_event(&mut event, ProducerSet::default());
                            let epoch = self.event_epoch.lock();
                            if let Some(candidate) = self.hook_candidate.as_mut() {
                                candidate.overflowed |= epoch.folded_hooks.as_ref().is_some_and(|folded| {
                                    !folded.represented_by_hook && folded.revision > self.claimed_revision
                                });
                            }
                            batch.rebuild_scope = if epoch.pending_site_rebuild.is_some_and(|revision| revision > self.claimed_revision)
                                || epoch.directory_refresh.is_some() {
                                RebuildScope::FullSite
                            } else {
                                event.paths.iter().filter_map(|path| epoch.pending.get(path))
                                    .fold(RebuildScope::Paths, |scope, pending| scope.max(pending.rebuild_scope))
                            };
                        }
                        batch.paths = event.paths.into_iter().filter(|path| rebuild_paths.accepts(path)).collect();
                        self.debouncer = Debouncer::new();
                        if self.hook_candidate.is_none() && self.suppress_observed_hook_changes(&mut batch).await? {
                            continue;
                        }
                        if !batch.paths.is_empty() || batch.rebuild_scope > RebuildScope::Paths {
                            return Ok(batch);
                        }
                        continue;
                    }
                    if !rescan && invalidated.is_empty() {
                        event.paths.retain(|path| !self.roots.classify(path).is_empty());
                        if event.paths.is_empty() {
                            continue;
                        }
                    }
                    if !rescan && self.quarantine_hook_event(&mut event, invalidated) {
                        continue;
                    }
                    if !invalidated.is_empty() && self.hook_candidate.is_some() {
                        self.debouncer = Debouncer::new();
                        return Ok(FileChangeBatch {
                            paths: event.paths.into_iter()
                                .filter(|path| rebuild_paths.accepts(path))
                                .collect(),
                            rebuild_scope: RebuildScope::FullSite,
                        });
                    }
                    self.debouncer.add_event(&event, std::time::Instant::now());
                }
                _ = async {
                    match remaining {
                        Some(remaining) => tokio::time::sleep(remaining).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
    }

    async fn install_selection(
        &mut self,
        selection: WatchSelection,
        retain_extension_refresh: bool,
    ) -> Result<bool, WatchError> {
        let (mut update, extended, attached, refresh_epoch) = {
            let mut epoch = self.event_epoch.lock();
            // Existing coverage repairs retain their triggering paths; only a
            // new input range requests an additional failed-read attempt.
            let extended = self.roots.extends_observation(&selection);
            let update = self
                .roots
                .directory_update(&selection, epoch.directory_refresh.is_some());
            // Candidate reads are revalidated after accepting this logical union
            // and establishing any newly required physical coverage.
            self.roots.commit_logical(selection.clone(), epoch.revision);
            epoch.boundary = self.roots.boundary();
            let refresh_epoch = epoch.revision;
            let attached = !update.retains_directories();
            if attached {
                epoch.directory_refresh.get_or_insert(refresh_epoch);
            }
            (update, extended, attached, refresh_epoch)
        };
        if attached {
            let applied = self
                .observer
                .as_mut()
                .expect("observation is open before shutdown")
                .update(update.take_operations())
                .await;
            if let Err(error) = applied {
                return Err(match error.origin {
                    Some(notify::PathOp::Watch(path, _)) => WatchError::Attach {
                        path,
                        source: error.source,
                    },
                    _ => WatchError::Create(error.source),
                });
            }
        }
        {
            let mut epoch = self.event_epoch.lock();
            let installed_coverage = self.roots.has_installed_directories();
            self.roots.commit(selection, update, epoch.revision);
            epoch.boundary = self.roots.boundary();
            // A callback during attachment can invalidate the directories again, and only the
            // invalidation covered by this update is repaired. A newly attached watch missed what
            // happened while it attached, so it keeps the marker; the first attachment clears it.
            let extended_attachment =
                attached && extended && installed_coverage && retain_extension_refresh;
            if !extended_attachment
                && epoch
                    .directory_refresh
                    .is_some_and(|revision| revision <= refresh_epoch)
            {
                epoch.directory_refresh = None;
            }
        }
        Ok(extended)
    }

    fn quarantine_hook_event(
        &mut self,
        event: &mut notify::Event,
        invalidated: ProducerSet,
    ) -> bool {
        let Some(candidate) = self.hook_candidate.as_mut() else {
            return false;
        };
        let epoch = self.event_epoch.lock();
        let mut has_hook_paths = false;
        event.paths.retain(|path| {
            if candidate
                .source_identities
                .iter()
                .any(|claim| claim.intersects_path(path))
            {
                has_hook_paths = true;
                if epoch.pending.contains_key(path)
                    && (candidate.paths.contains(path)
                        || candidate.paths.len() < EVENT_DETAIL_CAPACITY)
                {
                    candidate.paths.insert(path.clone());
                } else {
                    candidate.overflowed = true;
                }
                false
            } else {
                true
            }
        });
        if has_hook_paths && !invalidated.is_empty() {
            candidate.rebuild_scope = RebuildScope::FullSite;
        }
        has_hook_paths && event.paths.is_empty()
    }

    async fn observe_hook_paths(
        &mut self,
        paths: Vec<PathBuf>,
        outputs: &[tola_build::hooks::HookOutputEvidence],
        failed: Option<Arc<tola_build::hooks::FailedHookOutputs>>,
        cancellation: tola_build::cancellation::BuildCancellation,
    ) -> Result<ConfirmedHookPaths, WatchError> {
        cancellation
            .ensure_active()
            .map_err(|error| WatchError::HookInputs(error.into()))?;
        let folded = self
            .event_epoch
            .lock()
            .folded_hooks
            .as_ref()
            .filter(|folded| !folded.represented_by_hook)
            .cloned();
        let relevant = |covers: &dyn Fn(&std::path::Path) -> bool| {
            paths.iter().any(|path| covers(path))
                || folded.as_ref().is_some_and(|folded| {
                    folded
                        .roots()
                        .any(|root| covers(root.logical_path()) || covers(root.physical_path()))
                })
        };
        let outputs = outputs
            .iter()
            .filter(|output| relevant(&|path| output.covers_path(path)))
            .cloned()
            .collect::<Vec<_>>();
        let failed = failed.filter(|failed| relevant(&|path| failed.covers_path(path)));
        if outputs.is_empty() && failed.is_none() {
            return Ok(ConfirmedHookPaths::default());
        }
        assert!(
            self.hook_path_check.is_none(),
            "generated input observation has one owner"
        );
        let canceller = tola_build::cancellation::BuildCanceller::new();
        let worker_cancellation = canceller.token();
        let worker = tokio::task::spawn_blocking(move || {
            let mut confirmed = std::collections::BTreeSet::new();
            let mut current_outputs = Vec::new();
            for output in outputs {
                if output.is_current_with_cancellation(&worker_cancellation)? {
                    confirmed.extend(
                        paths
                            .iter()
                            .filter(|path| output.covers_path(path))
                            .cloned(),
                    );
                    current_outputs.push(output);
                }
            }
            if let Some(failed) = &failed
                && let Some(current) = failed.current_for_paths(&paths, &worker_cancellation)?
            {
                confirmed.extend(
                    paths
                        .iter()
                        .filter(|path| current.covers_path(path))
                        .cloned(),
                );
            }
            let mut folded_revision = None;
            if let Some(folded) = folded {
                let mut covered = true;
                for root in folded.roots() {
                    worker_cancellation.ensure_active()?;
                    if current_outputs.iter().any(|output| {
                        output.covers_path(root.logical_path())
                            || output.covers_path(root.physical_path())
                    }) {
                        continue;
                    }
                    let paths = [
                        root.logical_path().to_path_buf(),
                        root.physical_path().to_path_buf(),
                    ];
                    let current = match &failed {
                        Some(failed) => failed.current_for_paths(&paths, &worker_cancellation)?,
                        None => None,
                    };
                    if !current
                        .is_some_and(|current| paths.iter().any(|path| current.covers_path(path)))
                    {
                        covered = false;
                        break;
                    }
                }
                if covered {
                    folded_revision = Some(folded.revision);
                }
            }
            worker_cancellation.ensure_active()?;
            Ok(ConfirmedHookPaths {
                paths: confirmed,
                folded_revision,
            })
        });
        self.hook_path_check = Some(RunningHookPathCheck { canceller, worker });
        let observed = (&mut self
            .hook_path_check
            .as_mut()
            .expect("observation is owned until it completes")
            .worker)
            .await;
        self.hook_path_check = None;
        cancellation
            .ensure_active()
            .map_err(|error| WatchError::HookInputs(error.into()))?;
        observed
            .map_err(|error| WatchError::HookInputs(error.into()))?
            .map_err(WatchError::HookInputs)
    }

    async fn suppress_observed_hook_changes(
        &mut self,
        batch: &mut FileChangeBatch,
    ) -> Result<bool, WatchError> {
        if self
            .event_epoch
            .lock()
            .folded_hooks
            .as_ref()
            .is_none_or(|folded| folded.represented_by_hook)
            && !self
                .source_hooks
                .outputs()
                .iter()
                .any(|output| batch.paths.iter().any(|path| output.covers_path(path)))
            && !self
                .failed_hook_outputs
                .as_ref()
                .is_some_and(|failed| batch.paths.iter().any(|path| failed.covers_path(path)))
        {
            return Ok(false);
        }
        // Only a generated-output event needs repaired coverage before its
        // evidence can suppress the batch.
        self.refresh_pending().await?;
        let source_hooks = self.source_hooks.clone();
        let failed = self.failed_hook_outputs.clone();
        let confirmed = self
            .observe_hook_paths(
                batch.paths.clone(),
                source_hooks.outputs(),
                failed,
                tola_build::cancellation::BuildCancellation::new(),
            )
            .await?;
        let mut epoch = self.event_epoch.lock();
        batch.paths.retain(|path| {
            if !confirmed.confirms(path) {
                return true;
            }
            if let Some(pending) = epoch.pending.get_mut(path) {
                pending.represented_by_hook = true;
            }
            false
        });
        if let Some(folded) = epoch.folded_hooks.as_mut()
            && confirmed.folded_revision == Some(folded.revision)
        {
            folded.represented_by_hook = true;
        }
        drop(epoch);
        // A repaired directory's event no longer widens an unrelated edit's
        // scope once its generated contents have been observed unchanged.
        batch.rebuild_scope = self
            .event_epoch
            .changes_after(self.claimed_revision)
            .rebuild_scope;
        Ok(batch.paths.is_empty() && batch.rebuild_scope == RebuildScope::Paths)
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), WatchError> {
        self.event_epoch.lock().boundary = EventBoundary::default();
        self.event_tx.close();
        let observation = if let Some(mut observation) = self.hook_path_check.take() {
            observation.canceller.cancel();
            (&mut observation.worker)
                .await
                .map(|_| ())
                .map_err(|error| WatchError::HookInputs(error.into()))
        } else {
            Ok(())
        };
        if let Some(observer) = self.observer.take() {
            let stopped = observer.shutdown().await.map_err(WatchError::Create);
            observation.and(stopped)?;
        } else {
            observation?;
        }
        Ok(())
    }
}

/// Every generated path a prepared hook watch declares, as a watch source identity.
fn generated_source_identities(
    inputs: &tola_module::HookWatchInputs,
) -> impl Iterator<Item = tola_input::FilesystemSourceIdentity> + '_ {
    inputs
        .generated_paths()
        .iter()
        .map(|path| tola_input::FilesystemSourceIdentity::from_path(path))
}

fn forward_watch_result<S: WatchMessageSink>(
    mut event: notify::Result<notify::Event>,
    event_epoch: &EventEpoch,
    events: &S,
    log_file: Option<&Path>,
) {
    if let (Ok(event), Some(log_file)) = (&mut event, log_file)
        && !event.need_rescan()
    {
        event
            .paths
            .retain(|path| tola_input::normalize_existing_prefix(path).as_path() != log_file);
        if event.paths.is_empty() {
            return;
        }
    }
    // Logical subscription changes, callback acceptance, and revision replacement share
    // this lock. The native observer never runs or shuts down while it is held.
    let mut epoch = event_epoch.lock();
    let producers = epoch.boundary.producers();
    if producers.is_empty() {
        return;
    }
    let event = match event {
        Ok(event) => event,
        Err(error) => {
            dispatch_watch_message(
                WatchMessage::Error {
                    producers,
                    error: Arc::new(error),
                },
                &mut epoch,
                events,
            );
            return;
        }
    };
    let boundary = epoch.boundary.clone();
    let rescan = event.need_rescan();
    let hook_rescan = rescan
        && !event.paths.is_empty()
        && event::normalized_event_paths(&event)
            .filter_map(|path| {
                let classified = boundary.classify_change(&path, event.kind);
                (!classified.is_empty()).then_some(classified)
            })
            .all(ClassifiedPath::is_hook_output)
        && event::normalized_event_paths(&event)
            .any(|path| !boundary.classify_change(&path, event.kind).is_empty());
    if rescan && !hook_rescan {
        dispatch_watch_message(
            WatchMessage::Event {
                event: notify::Event::new(notify::EventKind::Other)
                    .set_flag(notify::event::Flag::Rescan),
                revision: 0,
                classified_paths: Vec::new(),
                producers,
                invalidated: producers,
            },
            &mut epoch,
            events,
        );
        return;
    }
    let invalidated = if hook_rescan {
        ProducerSet::default()
    } else {
        boundary.invalidated_by(&event)
    };
    if invalidated.is_empty() && !event::accepts_event(event.kind) && !hook_rescan {
        return;
    }
    let mut read_budget = COMMITTED_CONTENT_READ_BUDGET;
    let classified = event::normalized_event_paths(&event)
        .filter(|path| {
            rescan
                || !invalidated.is_empty()
                || !is_editor_temp(path)
                || boundary.admits_editor_artifact(path, event::is_structural(event.kind))
        })
        .filter_map(|path| {
            let classified = if invalidated.is_empty() {
                boundary.classify_change(&path, event.kind)
            } else {
                ClassifiedPath::new(invalidated, RebuildScope::FullSite)
            };
            if classified.is_empty() {
                return None;
            }
            // A redelivered event is settled only after a committed claim, when the path still
            // holds the content that claim captured; declared hook outputs keep their own
            // evidence path, and anything unproven keeps its event.
            if invalidated.is_empty()
                && !rescan
                && !classified.is_hook_output()
                && epoch.claim_committed
                && epoch.holds_committed_content(&path, &mut read_budget)
            {
                tracing::debug!(
                    target: "tola::watch",
                    path = ?path,
                    "dropped a filesystem event the installed revision already reflects"
                );
                return None;
            }
            Some((path, classified))
        })
        .collect::<Vec<_>>();
    for classified_paths in classified.chunks(EVENT_DETAIL_CAPACITY) {
        let mut accepted = notify::Event::new(event.kind);
        accepted.paths = classified_paths
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        if hook_rescan {
            accepted = accepted.set_flag(notify::event::Flag::Rescan);
        }
        let producers = classified_paths.iter().fold(
            ProducerSet::default(),
            |mut producers, (_, classified)| {
                producers.insert(classified.producers());
                producers
            },
        );
        dispatch_watch_message(
            WatchMessage::Event {
                event: accepted,
                revision: 0,
                classified_paths: classified_paths.to_vec(),
                producers,
                invalidated,
            },
            &mut epoch,
            events,
        );
    }
}

fn dispatch_watch_message<S: WatchMessageSink>(
    mut message: WatchMessage,
    epoch: &mut EventEpochState,
    events: &S,
) {
    match &message {
        WatchMessage::Event {
            event,
            classified_paths,
            producers,
            invalidated,
            ..
        } => {
            // Every path is a declared hook output, so the rescan describes the hook's own write
            // and owes no rebuild: a path list has that write, and consuming it is what keeps
            // the session from rebuilding for the round that just wrote it.
            let hook_outputs_only = event.need_rescan()
                && !classified_paths.is_empty()
                && classified_paths
                    .iter()
                    .all(|(_, classified)| classified.is_hook_output());
            if event.need_rescan() && !hook_outputs_only {
                epoch.advance_site_rebuild(*producers);
            } else {
                epoch.advance_classified(classified_paths.iter().cloned());
                // A declared hook output is not a structural change: its write moves no tree a
                // subscription still owes.
                if event::is_structural(event.kind) {
                    for (path, classified) in classified_paths {
                        if classified.is_hook_output() {
                            continue;
                        }
                        if let Some(pending) = epoch.pending.get_mut(path) {
                            pending.structural = true;
                        }
                    }
                }
                if !invalidated.is_empty() {
                    epoch.invalidate_directories(*invalidated);
                }
            }
        }
        WatchMessage::Error { producers, error } => epoch.fail(*producers, Arc::clone(error)),
    }
    if let WatchMessage::Event { revision, .. } = &mut message {
        *revision = epoch.revision;
    }
    if events.send_message(message).is_some() {
        events.mark_overflow_ready(epoch.revision);
    }
}

fn take_rebuild_batch(
    debouncer: &mut Debouncer,
    rebuild_paths: &RebuildPathFilter,
    rebuild_scope: RebuildScope,
    now: std::time::Instant,
) -> Option<FileChangeBatch> {
    let paths = debouncer.take_if_ready(now, |path| rebuild_paths.accepts(path))?;
    (!paths.is_empty() || rebuild_scope > RebuildScope::Paths).then_some(FileChangeBatch {
        paths,
        rebuild_scope,
    })
}

/// Roots excluded from development rebuilds for one resolved configuration; one filter per
/// configuration attempt resolves the excluded roots once instead of once per candidate path.
struct RebuildPathFilter {
    excluded: [Option<tola_input::FilesystemSourceIdentity>; 5],
}

impl RebuildPathFilter {
    fn for_config(config: &ResolvedSiteConfig) -> Self {
        use tola_build::filesystem::{SITE_BUILD_LOCK_FILE, swap_workspace};
        use tola_input::{FilesystemSourceIdentity, INTERNAL_DIR};
        let root = config.get_root();
        Self {
            excluded: [
                Some(config.build().publish_dir.clone()),
                Some(root.join(INTERNAL_DIR)),
                Some(root.join(SITE_BUILD_LOCK_FILE)),
                swap_workspace(&config.build().publish_dir),
                config.vendor().path.as_deref().and_then(swap_workspace),
            ]
            .map(|path| path.map(|path| FilesystemSourceIdentity::from_path(&path))),
        }
    }

    fn accepts(&self, path: &Path) -> bool {
        let path = tola_input::FilesystemSourceIdentity::from_path(path);
        !self.excluded.iter().flatten().any(|boundary| {
            [path.logical_path(), path.physical_path()]
                .into_iter()
                .any(|path| {
                    path.starts_with(boundary.logical_path())
                        || path.starts_with(boundary.physical_path())
                })
        })
    }
}

fn hook_output_source_identities(
    config: &ResolvedSiteConfig,
) -> Vec<tola_input::FilesystemSourceIdentity> {
    roots::development_before_build_hooks(config)
        .flat_map(|hook| hook.generates.iter())
        .map(|path| tola_input::FilesystemSourceIdentity::from_path(&config.get_root().join(path)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc;
    use tola_build::build::InputPathScope;

    impl WatchMessageSink for mpsc::UnboundedSender<WatchMessage> {
        fn send_message(&self, message: WatchMessage) -> Option<ProducerSet> {
            let _ = self.send(message);
            None
        }
    }

    impl EventEpoch {
        /// The guard one candidate holds while it verifies; the tests that keep a claim across
        /// an event use it to prove an unrelated event does not discard the candidate.
        fn guard_relevant(
            &self,
            expected: u64,
            relevant: impl FnMut(&Path) -> bool,
        ) -> Result<Option<EventEpochStateGuard<'_>>, Arc<notify::Error>> {
            self.guard_relevant_confirmed(expected, relevant, &ConfirmedHookPaths::default())
        }
    }

    /// Executed as an independent hook child, never directly by the test harness.
    #[test]
    fn child_writes_nothing() {}

    pub(super) fn resolve_schema(
        root: &Path,
        schema: tola_build::config::SiteConfigSchema,
        packages: tola_typst::PackageLocations,
    ) -> ResolvedSiteConfig {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join("tola.toml");
        if !path.exists() {
            std::fs::write(&path, toml::to_string(&schema).unwrap()).unwrap();
        }
        schema
            .resolve(
                &path,
                packages,
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap()
    }

    pub(super) fn local_packages() -> tola_typst::PackageLocations {
        tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap()
    }

    fn watch_config(root: &Path) -> ResolvedSiteConfig {
        let content = root.join("content");
        std::fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        if !entry.exists() {
            std::fs::write(&entry, "#document(\"index.html\")[Site]").unwrap();
        }
        resolve_schema(
            root,
            tola_build::config::SiteConfigSchema::default(),
            local_packages(),
        )
    }

    async fn manual_watch_source(
        config: &ResolvedSiteConfig,
        source_hooks: &tola_build::hooks::SourceHookOutputs,
    ) -> FileChangeSource {
        let (event_tx, events) = WatchEventQueue::channel();
        let roots = RootSet::from_config(config, &WatchRequirements::default()).unwrap();
        let observer = DirectoryObserver::start(|| Ok(notify::NullWatcher))
            .await
            .unwrap();
        let mut changes = FileChangeSource {
            event_tx,
            events,
            event_epoch: EventEpoch::default(),
            claimed_revision: 0,
            observer: Some(observer),
            roots,
            debouncer: Debouncer::new(),
            hook_candidate: None,
            source_hooks: source_hooks.clone(),
            failed_hook_outputs: None,
            hook_path_check: None,
        };
        changes
            .install_selection(changes.roots.initial_selection(), false)
            .await
            .unwrap();
        changes.roots.confirm_current_observation();
        changes
    }

    #[tokio::test]
    async fn shutdown_cancels_observation_worker() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = watch_config(directory.path());
        let mut changes = manual_watch_source(&config, &Default::default()).await;
        let canceller = tola_build::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let worker_cancellation = cancellation.clone();
        let (release, released) = std::sync::mpsc::channel();
        let (completed, completion) = std::sync::mpsc::channel();
        changes.hook_path_check = Some(RunningHookPathCheck {
            canceller,
            worker: tokio::task::spawn_blocking(move || {
                released.recv().unwrap();
                let observed = worker_cancellation.ensure_active();
                completed.send(()).unwrap();
                observed?;
                Ok(Default::default())
            }),
        });
        let closing = changes.shutdown();
        tokio::pin!(closing);
        assert!(
            std::future::poll_fn(|task| {
                std::task::Poll::Ready(
                    std::future::Future::poll(closing.as_mut(), task).is_pending(),
                )
            })
            .await
        );
        assert!(cancellation.is_cancelled());

        release.send(()).unwrap();
        closing.await.unwrap();

        completion
            .try_recv()
            .expect("shutdown joined its observation worker");
    }

    #[test]
    fn callback_errors_reach_the_consumer() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = watch_config(directory.path());
        let boundary = RootSet::from_config(&config, &WatchRequirements::default())
            .unwrap()
            .initial_selection()
            .boundary();
        let expected_producers = boundary.producers();
        let epoch = EventEpoch::default();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        epoch.lock().boundary = boundary;

        forward_watch_result(
            Err(notify::Error::generic("injected callback failure")),
            &epoch,
            &events_tx,
            None,
        );

        let WatchMessage::Error { producers, error } = events_rx.try_recv().unwrap() else {
            panic!("callback error must remain an error in the event stream");
        };
        assert_eq!(producers, expected_producers);
        assert_eq!(error.to_string(), "injected callback failure");
        assert_eq!(epoch.revision(), 1);
        assert!(epoch.guard_relevant(0, |_| true).is_err());
    }

    #[test]
    fn log_file_exclusion_spares_siblings() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = watch_config(directory.path());
        let log_file = config.get_root().join("tola.log");
        let source = config.get_root().join("source.log");
        std::fs::write(&log_file, "session log").unwrap();
        std::fs::write(&source, "site input").unwrap();
        let log_file = std::fs::canonicalize(log_file).unwrap();
        let mut required = WatchRequirements::default();
        required.observe_paths(
            ProducerKind::TypstReads,
            InputPathScope::Recursive,
            true,
            [config.get_root().to_path_buf()],
        );
        let epoch = watching_epoch(&config, &required);
        let (events, mut received) = mpsc::unbounded_channel();

        forward_watch_result(
            Ok(modified_event(&log_file)),
            &epoch,
            &events,
            Some(&log_file),
        );
        assert_eq!(epoch.revision(), 0);
        assert!(received.try_recv().is_err());
        #[cfg(unix)]
        {
            let alias = config.get_root().join("log-link");
            std::os::unix::fs::symlink(&log_file, &alias).unwrap();
            forward_watch_result(Ok(modified_event(alias)), &epoch, &events, Some(&log_file));
            assert_eq!(epoch.revision(), 0);
            assert!(received.try_recv().is_err());
        }

        forward_watch_result(
            Ok(modified_event(&log_file).add_path(source.clone())),
            &epoch,
            &events,
            Some(&log_file),
        );
        let WatchMessage::Event { event, .. } = received.try_recv().unwrap() else {
            panic!("other paths in the event must remain observable");
        };
        assert_eq!(event.paths, [source]);

        forward_watch_result(
            Ok(modified_event(&log_file).set_flag(notify::event::Flag::Rescan)),
            &epoch,
            &events,
            Some(&log_file),
        );
        let WatchMessage::Event { event, .. } = received.try_recv().unwrap() else {
            panic!("an observation gap must retain its rescan request");
        };
        assert!(event.need_rescan());
        assert!(epoch.requires_directory_refresh());
        assert_eq!(
            epoch.claim_candidate().rebuild_scope(),
            RebuildScope::FullSite
        );
    }

    #[test]
    fn configured_tree_member_reaches_the_rebuild() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let mut schema = tola_build::config::SiteConfigSchema::default();
        schema
            .assets
            .trees
            .push(tola_build::config::section::AssetTreeDeclaration::new(
                "assets",
                tola_build::config::section::AssetUrlPrefix::parse("/assets").unwrap(),
            ));
        let config = resolve_schema(root, schema, local_packages());
        std::fs::create_dir_all(config.get_root().join("assets")).unwrap();
        std::fs::create_dir_all(config.get_root().join("static/typst-fonts")).unwrap();
        let required = WatchRequirements::default();

        for (member, producer) in [
            // A configured asset tree publishes every member it holds.
            (
                PathBuf::from("assets/data.tmp"),
                ProducerKind::ConfiguredAssets,
            ),
            // A configured font directory retains an accepted member and its bytes.
            (
                PathBuf::from("static/typst-fonts/.#Nimbus.otf"),
                ProducerKind::Fonts,
            ),
        ] {
            let member = config.get_root().join(&member);
            let epoch = EventEpoch::default();
            epoch.lock().boundary = RootSet::from_config(&config, &required)
                .unwrap()
                .initial_selection()
                .boundary();
            let (events, mut received) = mpsc::unbounded_channel();

            forward_watch_result(Ok(modified_event(&member)), &epoch, &events, None);

            let WatchMessage::Event {
                event, producers, ..
            } = received.try_recv().unwrap()
            else {
                panic!("{} must stay observable", member.display());
            };
            assert_eq!(event.paths, [member]);
            assert_eq!(producers, ProducerSet::one(producer));
        }
    }

    fn modified_event(path: impl Into<PathBuf>) -> notify::Event {
        notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(path.into())
    }

    fn watching_epoch(config: &ResolvedSiteConfig, required: &WatchRequirements) -> EventEpoch {
        let epoch = EventEpoch::default();
        epoch.lock().boundary = RootSet::from_config(config, required)
            .unwrap()
            .initial_selection()
            .boundary();
        epoch
    }

    fn deliver_event(changes: &mut FileChangeSource, event: &notify::Event) {
        forward_watch_result(
            Ok(event.clone()),
            &changes.event_epoch,
            &changes.event_tx,
            None,
        );
        changes.debouncer.add_event(
            event,
            std::time::Instant::now()
                - std::time::Duration::from_millis(debouncer::MAX_BATCH_WAIT_MS + 1),
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_watch_tracks_tool_and_output() {
        use tola_build::build::{BuildMode, BuildRequest, BuildSession};
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), "#document(\"index.html\")[Site]").unwrap();
        let module = root.join("module");
        std::fs::create_dir_all(module.join("tools")).unwrap();
        let tool = module.join("tools/input.txt");
        std::fs::write(&tool, "first").unwrap();
        std::fs::write(
            module.join("tola-module.toml"),
            r#"

[[hooks.before-build]]
name = "generate"
argv = ["sh", "-c", "mkdir -p generated; printf generated > generated/notes.txt"]
rerun-on = { include = ["tools/*.txt"] }
generates = ["generated/{name}.txt"]
"#,
        )
        .unwrap();
        let config = tola_build::config::loading::resolve_site_config(
            &root.join("tola.toml"),
            "[modules.notes]\npath = \"module\"\n",
            local_packages(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let hook_watch = tola_build::modules::prepare_hook_watch(
            &config,
            tola_input::InputMode::new(tola_input::InputScope::Online, false),
            &Default::default(),
        )
        .unwrap();
        let mut changes = manual_watch_source(&config, &Default::default()).await;
        let claim = changes.begin_candidate(&config);
        changes.update_hook_candidate_claims(hook_watch.config());
        changes.extend_hook_candidate_claims(hook_watch.inputs());
        let mut dynamic = WatchRequirements::default();
        dynamic.add_module_hook_inputs(hook_watch.inputs());
        changes
            .stage_config(hook_watch.config(), &dynamic)
            .await
            .unwrap();
        changes.refresh_pending().await.unwrap();
        let mut session = BuildSession::new();
        let built = session
            .prepare_with_hook_watch(&hook_watch, BuildRequest::new(BuildMode::Development))
            .unwrap()
            .run()
            .unwrap();
        let output = root.join("generated/notes.txt");
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "generated");
        forward_watch_result(
            Ok(modified_event(&output)),
            &changes.event_epoch,
            &changes.event_tx,
            None,
        );
        let actual_config = Arc::clone(built.config());
        let source_hooks = built.source_hooks().clone();
        let subscriptions = changes
            .stage_config(
                &actual_config,
                &crate::dev::watch::watch_requirements_from_observation(&built.input_observation()),
            )
            .await
            .unwrap();
        assert!(changes.event_epoch.lock().pending.contains_key(&output));
        // Promote watches for the directory the hook created; never discard its event.
        changes.refresh_pending().await.unwrap();
        assert!(changes.event_epoch.lock().pending.contains_key(&output));
        let guard = changes
            .guard_epoch(
                claim.revision(),
                &actual_config,
                source_hooks.outputs(),
                &tola_build::cancellation::BuildCancellation::default(),
            )
            .await
            .unwrap()
            .expect("confirmed generator output must not reject the candidate");
        guard.commit(claim, subscriptions);
        changes.complete_hook_candidate(&source_hooks);
        assert!(changes.event_epoch.claim_candidate().paths.is_empty());

        std::fs::write(&tool, "changed").unwrap();
        let event = modified_event(&tool);
        deliver_event(&mut changes, &event);
        let batch = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            changes.next(&actual_config),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(batch.paths.contains(&tool), "{batch:?}");
        changes.shutdown().await.unwrap();
    }

    /// The failed chain re-runs for its own declared output only while the recorded bytes hold.
    fn settled_batch(
        config: &ResolvedSiteConfig,
        events: impl IntoIterator<Item = notify::Event>,
        rebuild_scope: RebuildScope,
    ) -> Option<FileChangeBatch> {
        let now = std::time::Instant::now();
        let mut debouncer = Debouncer::new();
        for event in events {
            debouncer.add_event(&event, now);
        }
        take_rebuild_batch(
            &mut debouncer,
            &RebuildPathFilter::for_config(config),
            rebuild_scope,
            now + std::time::Duration::from_millis(debouncer::QUIET_PERIOD_MS),
        )
    }

    #[test]
    fn ignores_generated_output_events() {
        let directory = tempfile::TempDir::new().unwrap();
        let configured = watch_config(directory.path());
        let root = configured.get_root().to_path_buf();
        let config = &configured;
        assert!(
            settled_batch(
                config,
                [
                    modified_event(root.join("public/index.html")),
                    modified_event(root.join(".tola-build.lock")),
                    modified_event(root.join(".public-swap/staged/0/index.html")),
                ],
                RebuildScope::Paths,
            )
            .is_none()
        );
        let batch = settled_batch(
            config,
            [
                modified_event(root.join("public/index.html")),
                modified_event(root.join("content/index.typ")),
            ],
            RebuildScope::Paths,
        )
        .unwrap();
        assert_eq!(batch.paths, vec![root.join("content/index.typ")]);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_alias_excludes_output_paths() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::TempDir::new().unwrap();
        let physical_root = directory.path().join("site");
        let alias_root = directory.path().join("site-alias");
        std::fs::create_dir(&physical_root).unwrap();
        symlink(&physical_root, &alias_root).unwrap();

        let config = watch_config(&alias_root);

        let filter = RebuildPathFilter::for_config(&config);
        let mirror = tola_input::layout::builtin_packages(&alias_root);
        assert!(!filter.accepts(&alias_root.join("public/index.html")));
        assert!(!filter.accepts(&mirror.join("cache")));
        assert!(!filter.accepts(&alias_root.join(".tola-build.lock")));
        assert!(!filter.accepts(&alias_root.join(".public-swap/previous/0/index.html")));
        let packages =
            tola_typst::PackageLocations::from_absolute_roots(None, Some(mirror.clone())).unwrap();
        let config = resolve_schema(
            &alias_root,
            tola_build::config::SiteConfigSchema::default(),
            packages,
        );
        let filter = RebuildPathFilter::for_config(&config);
        assert!(!filter.accepts(&mirror.join("cache")));
        assert!(filter.accepts(&alias_root.join("content/index.typ")));
    }
}
