use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_build::config::SiteConfig;

use super::boundary::{CoverageCertainty, EventBoundary, WatchSelection};
use super::debouncer::Debouncer;
use super::dispatch::{
    RebuildPathFilter, forward_watch_result, generated_source_identities,
    hook_output_source_identities, take_rebuild_batch,
};
use super::epoch::{ConfirmedHookPaths, EventClaim, EventEpoch, EventGuard, capture_file_content};
use super::observer::DirectoryObserver;
use super::producer::{HookWatch, ProducerKind, ProducerSet, WatchRequirements};
use super::queue::{WatchEventQueue, WatchEventReceiver, WatchMessage};
use super::roots::RootSet;
use super::{EVENT_DETAIL_CAPACITY, FileChangeBatch, RebuildScope, WatchError};

/// Input subscriptions and directory watching for one development session.
pub(crate) struct FileChangeSource {
    pub(super) event_tx: WatchEventQueue,
    pub(super) events: WatchEventReceiver,
    pub(super) event_epoch: EventEpoch,
    pub(super) claimed_revision: u64,
    pub(super) observer: Option<DirectoryObserver>,
    pub(super) roots: RootSet,
    pub(super) debouncer: Debouncer,
    pub(super) hook_candidate: Option<QuarantinedHookPaths>,
    pub(super) source_hooks: tola_build::hooks::SourceHookOutputs,
    pub(super) failed_hook_outputs: Option<Arc<tola_build::hooks::FailedHookOutputs>>,
    pub(super) hook_path_check: Option<RunningHookPathCheck>,
}

pub(super) struct RunningHookPathCheck {
    pub(super) canceller: tola_build::cancellation::BuildCanceller,
    pub(super) worker: tokio::task::JoinHandle<anyhow::Result<ConfirmedHookPaths>>,
}

pub(super) struct QuarantinedHookPaths {
    source_identities: Vec<tola_input::FilesystemSourceIdentity>,
    paths: BTreeSet<PathBuf>,
    rebuild_scope: RebuildScope,
    overflowed: bool,
}

impl QuarantinedHookPaths {
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

impl FileChangeSource {
    pub(crate) async fn start(
        cfg: &SiteConfig,
        dynamic: &WatchRequirements,
        source_hooks: &tola_build::hooks::SourceHookOutputs,
        log_file: Option<&Path>,
    ) -> Result<Self, WatchError> {
        let (event_tx, events) = WatchEventQueue::channel();
        let event_epoch = EventEpoch::for_root(cfg.root());
        let roots = RootSet::from_config(cfg, dynamic).map_err(WatchError::RerunOn)?;
        let callback_epoch = event_epoch.clone();
        let callback_events = event_tx.clone();
        let log_file = log_file.map(tola_input::normalize_existing_prefix);
        let observer = DirectoryObserver::start(move || {
            notify::recommended_watcher(move |ev| {
                forward_watch_result(ev, &callback_epoch, &callback_events, log_file.as_deref());
            })
        })
        .await
        .map_err(WatchError::Create)?;
        let mut src = Self {
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
        if let Err(err) = src
            .install_selection(src.roots.initial_selection(), false)
            .await
        {
            src.shutdown().await?;
            return Err(err);
        }
        Ok(src)
    }

    pub(crate) async fn stage_config(
        &mut self,
        cfg: &SiteConfig,
        dynamic: &WatchRequirements,
    ) -> Result<WatchSelection, WatchError> {
        let (observing, subscriptions, retain) = self
            .roots
            .staged_selection(cfg, dynamic)
            .map_err(WatchError::RerunOn)?;
        self.install_selection(observing, retain).await?;
        Ok(subscriptions)
    }

    pub(crate) async fn reconfigure_configs<'a>(
        &mut self,
        configs: impl IntoIterator<Item = &'a SiteConfig>,
        dynamic: &WatchRequirements,
    ) -> Result<bool, WatchError> {
        let (sel, retain) = self
            .roots
            .configs_selection(configs, dynamic)
            .map_err(WatchError::RerunOn)?;
        self.install_selection(sel, retain).await
    }

    pub(crate) async fn refresh_pending(&mut self) -> Result<(), WatchError> {
        if self.event_epoch.requires_directory_refresh() {
            self.install_selection(self.roots.current_selection(), false)
                .await?;
        }
        Ok(())
    }

    pub(crate) fn begin_candidate(&mut self, cfg: &SiteConfig) -> EventClaim {
        self.hook_candidate = Some(QuarantinedHookPaths {
            source_identities: hook_output_source_identities(cfg),
            paths: BTreeSet::new(),
            rebuild_scope: RebuildScope::Paths,
            overflowed: false,
        });
        let rebuild_paths = RebuildPathFilter::for_config(cfg);
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

    pub(crate) fn update_hook_candidate_claims(&mut self, cfg: &SiteConfig) {
        if let Some(candidate) = self.hook_candidate.as_mut() {
            candidate.source_identities = hook_output_source_identities(cfg);
        }
    }

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
                    .is_none_or(|rev| rev <= self.claimed_revision)
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

    pub(crate) fn hook_watches(&self, paths: &[PathBuf]) -> Vec<HookWatch> {
        let epoch = self.event_epoch.lock();
        let mut watches = BTreeSet::new();
        for path in paths {
            let structural = epoch.pending.get(path).is_some_and(|ev| ev.structural);
            watches.extend(epoch.boundary.hook_watches(path, structural));
        }
        watches.into_iter().collect()
    }

    pub(crate) fn accepts_events_for(
        &self,
        producers: impl IntoIterator<Item = ProducerKind>,
    ) -> bool {
        !self.event_epoch.requires_directory_refresh()
            && producers.into_iter().all(|producer| {
                self.roots.coverage(producer).is_some_and(|coverage| {
                    coverage.certainty() == CoverageCertainty::AcceptedEvents
                        && coverage.covers_current_revision()
                        && !self
                            .event_epoch
                            .is_uncertain_after(producer, coverage.established_epoch())
                })
            })
    }

    pub(crate) fn rebuild_changes_after(&self, rev: u64, cfg: &SiteConfig) -> FileChangeBatch {
        let rebuild_paths = RebuildPathFilter::for_config(cfg);
        let mut changes = self.event_epoch.changes_after(rev);
        changes.paths.retain(|path| rebuild_paths.accepts(path));
        changes
    }

    pub(crate) async fn guard_epoch(
        &mut self,
        expected: u64,
        cfg: &SiteConfig,
        hook_outputs: &[tola_build::hooks::HookOutput],
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<Option<EventGuard<'_>>, WatchError> {
        if self.event_epoch.requires_directory_refresh() {
            return Ok(None);
        }
        let paths = self.event_epoch.changes_after(expected).paths;
        let rebuild_paths = RebuildPathFilter::for_config(cfg);
        let ignored = paths
            .iter()
            .filter(|path| !rebuild_paths.accepts(path))
            .cloned()
            .collect::<BTreeSet<_>>();
        let confirmed = self
            .observe_hook_paths(paths, hook_outputs, None, cancellation.clone())
            .await?;
        let Self {
            event_epoch, roots, ..
        } = self;
        event_epoch
            .guard_relevant_confirmed(expected, |path| !ignored.contains(path), &confirmed)
            .map(|guard| guard.map(|epoch| EventGuard { epoch, roots }))
            .map_err(WatchError::Event)
    }

    pub(crate) async fn next(&mut self, cfg: &SiteConfig) -> Result<FileChangeBatch, WatchError> {
        let rebuild_paths = RebuildPathFilter::for_config(cfg);
        loop {
            let now = std::time::Instant::now();
            let remaining = self.debouncer.remaining(now);
            if remaining.is_some_and(|rem| rem.is_zero()) {
                let newer = self.event_epoch.changes_after(self.claimed_revision);
                if let Some(mut batch) = take_rebuild_batch(
                    &mut self.debouncer,
                    &rebuild_paths,
                    newer.rebuild_scope,
                    now,
                ) {
                    batch.paths = newer
                        .paths
                        .into_iter()
                        .filter(|path| {
                            rebuild_paths.accepts(path) && !self.roots.classify(path).is_empty()
                        })
                        .collect();
                    if self.hook_candidate.is_none()
                        && self.suppress_observed_hook_changes(&mut batch).await?
                    {
                        continue;
                    }
                    if !batch.paths.is_empty() || batch.rebuild_scope > RebuildScope::Paths {
                        return Ok(batch);
                    }
                }
                continue;
            }
            tokio::select! {
                ev = self.events.recv() => {
                    let Some(ev) = ev else {
                        return Err(WatchError::Stopped);
                    };
                    let (mut ev, invalidated, rev) = match ev {
                        WatchMessage::Event { event, invalidated, revision, .. } => (event, invalidated, revision),
                        WatchMessage::Error { error, .. } => return Err(WatchError::Event(error)),
                    };
                    if rev <= self.claimed_revision {
                        continue;
                    }
                    let rescan = ev.need_rescan();
                    if rescan {
                        if let Some(batch) = self.handle_rescan(&mut ev, &rebuild_paths).await? {
                            return Ok(batch);
                        }
                        continue;
                    }
                    if invalidated.is_empty() {
                        ev.paths.retain(|path| !self.roots.classify(path).is_empty());
                        if ev.paths.is_empty() {
                            continue;
                        }
                    }
                    if self.quarantine_hook_event(&mut ev, invalidated) {
                        continue;
                    }
                    if !invalidated.is_empty() && self.hook_candidate.is_some() {
                        self.debouncer = Debouncer::new();
                        return Ok(FileChangeBatch {
                            paths: ev.paths.into_iter()
                                .filter(|path| rebuild_paths.accepts(path))
                                .collect(),
                            rebuild_scope: RebuildScope::FullSite,
                        });
                    }
                    self.debouncer.add_event(&ev, std::time::Instant::now());
                }
                _ = async {
                    match remaining {
                        Some(rem) => tokio::time::sleep(rem).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
    }

    async fn handle_rescan(
        &mut self,
        ev: &mut notify::Event,
        rebuild_paths: &RebuildPathFilter,
    ) -> Result<Option<FileChangeBatch>, WatchError> {
        let mut batch = self.event_epoch.changes_after(self.claimed_revision);
        ev.paths = std::mem::take(&mut batch.paths);
        if self.hook_candidate.is_some() {
            self.quarantine_hook_event(ev, ProducerSet::default());
            let epoch = self.event_epoch.lock();
            if let Some(candidate) = self.hook_candidate.as_mut() {
                candidate.overflowed |= epoch.folded_hooks.as_ref().is_some_and(|folded| {
                    !folded.represented_by_hook && folded.revision > self.claimed_revision
                });
            }
            batch.rebuild_scope = if epoch
                .pending_site_rebuild
                .is_some_and(|rev| rev > self.claimed_revision)
                || epoch.directory_refresh.is_some()
            {
                RebuildScope::FullSite
            } else {
                ev.paths
                    .iter()
                    .filter_map(|path| epoch.pending.get(path))
                    .fold(RebuildScope::Paths, |scope, pending| {
                        scope.max(pending.rebuild_scope)
                    })
            };
        }
        batch.paths = ev
            .paths
            .drain(..)
            .filter(|path| rebuild_paths.accepts(path))
            .collect();
        self.debouncer = Debouncer::new();
        if self.hook_candidate.is_none() && self.suppress_observed_hook_changes(&mut batch).await? {
            return Ok(None);
        }
        Ok((!batch.paths.is_empty() || batch.rebuild_scope > RebuildScope::Paths).then_some(batch))
    }

    pub(super) async fn install_selection(
        &mut self,
        sel: WatchSelection,
        retain_extension_refresh: bool,
    ) -> Result<bool, WatchError> {
        let (mut update, extended, attached, refresh_epoch) = {
            let mut epoch = self.event_epoch.lock();
            let extended = self.roots.extends_observation(&sel);
            let update = self
                .roots
                .directory_update(&sel, epoch.directory_refresh.is_some());
            self.roots.commit_logical(sel.clone(), epoch.revision);
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
            if let Err(err) = applied {
                return Err(match err.origin {
                    Some(notify::PathOp::Watch(path, _)) => WatchError::Attach {
                        path,
                        source: err.source,
                    },
                    _ => WatchError::Create(err.source),
                });
            }
        }
        {
            let mut epoch = self.event_epoch.lock();
            let installed_coverage = self.roots.has_installed_directories();
            self.roots.commit(sel, update, epoch.revision);
            epoch.boundary = self.roots.boundary();
            // A newly attached watch keeps its refresh marker until cleared; first attach clears it.
            let extended_attachment =
                attached && extended && installed_coverage && retain_extension_refresh;
            if !extended_attachment
                && epoch
                    .directory_refresh
                    .is_some_and(|rev| rev <= refresh_epoch)
            {
                epoch.directory_refresh = None;
            }
        }
        Ok(extended)
    }

    fn quarantine_hook_event(&mut self, ev: &mut notify::Event, invalidated: ProducerSet) -> bool {
        let Some(candidate) = self.hook_candidate.as_mut() else {
            return false;
        };
        let epoch = self.event_epoch.lock();
        let mut has_hook_paths = false;
        ev.paths.retain(|path| {
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
        has_hook_paths && ev.paths.is_empty()
    }

    async fn observe_hook_paths(
        &mut self,
        paths: Vec<PathBuf>,
        outputs: &[tola_build::hooks::HookOutput],
        failed: Option<Arc<tola_build::hooks::FailedHookOutputs>>,
        cancellation: tola_build::cancellation::BuildCancellation,
    ) -> Result<ConfirmedHookPaths, WatchError> {
        cancellation
            .ensure_active()
            .map_err(|err| WatchError::HookInputs(err.into()))?;
        let folded = self
            .event_epoch
            .lock()
            .folded_hooks
            .as_ref()
            .filter(|folded| !folded.represented_by_hook)
            .cloned();
        let relevant = |covers: &dyn Fn(&Path) -> bool| {
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
            let mut confirmed = BTreeSet::new();
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
                    let root_paths = [
                        root.logical_path().to_path_buf(),
                        root.physical_path().to_path_buf(),
                    ];
                    let current = match &failed {
                        Some(failed) => {
                            failed.current_for_paths(&root_paths, &worker_cancellation)?
                        }
                        None => None,
                    };
                    if !current
                        .is_some_and(|cur| root_paths.iter().any(|path| cur.covers_path(path)))
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
            .map_err(|err| WatchError::HookInputs(err.into()))?;
        observed
            .map_err(|err| WatchError::HookInputs(err.into()))?
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
        batch.rebuild_scope = self
            .event_epoch
            .changes_after(self.claimed_revision)
            .rebuild_scope;
        Ok(batch.paths.is_empty() && batch.rebuild_scope == RebuildScope::Paths)
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), WatchError> {
        self.event_epoch.lock().boundary = EventBoundary::default();
        self.event_tx.close();
        let observation = if let Some(mut check) = self.hook_path_check.take() {
            check.canceller.cancel();
            (&mut check.worker)
                .await
                .map(|_| ())
                .map_err(|err| WatchError::HookInputs(err.into()))
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
