use std::path::Path;
use std::sync::Arc;

use tola_build::config::SiteConfig;

use super::boundary::ClassifiedPath;
use super::debouncer::Debouncer;
use super::epoch::{COMMITTED_CONTENT_READ_BUDGET, EventEpoch, EventEpochState};
use super::event::{accepts_event, is_structural, normalized_event_paths};
use super::path::is_editor_temp;
use super::producer::ProducerSet;
use super::queue::{WatchMessage, WatchMessageSink};
use super::roots::development_before_build_hooks;
use super::{EVENT_DETAIL_CAPACITY, FileChangeBatch, RebuildScope};

pub(super) fn generated_source_identities(
    inputs: &tola_module::HookWatchInputs,
) -> impl Iterator<Item = tola_input::FilesystemSourceIdentity> + '_ {
    inputs
        .generated_paths()
        .iter()
        .map(|path| tola_input::FilesystemSourceIdentity::from_path(path))
}

pub(super) fn forward_watch_result<S: WatchMessageSink>(
    ev: notify::Result<notify::Event>,
    event_epoch: &EventEpoch,
    events: &S,
    log_file: Option<&Path>,
) {
    // Logical subscription changes, callback acceptance, and revision replacement share this lock.
    let mut epoch = event_epoch.lock();
    let producers = epoch.boundary.producers();
    if producers.is_empty() {
        return;
    }
    let mut ev = match ev {
        Ok(ev) => ev,
        Err(err) => {
            dispatch_watch_message(
                WatchMessage::Error {
                    producers,
                    error: Arc::new(err),
                },
                &mut epoch,
                events,
            );
            return;
        }
    };
    if !ev.paths.is_empty() {
        ev.paths.retain(|path| epoch.internal_paths.accepts(path));
        if ev.paths.is_empty() {
            return;
        }
    }
    // Reject known internal prefixes before resolving log aliases to avoid canonicalizing
    // thousands of temporary hook files on the native callback thread.
    if let Some(log_file) = log_file
        && !ev.need_rescan()
    {
        ev.paths
            .retain(|path| tola_input::normalize_existing_prefix(path).as_path() != log_file);
        if ev.paths.is_empty() {
            return;
        }
    }
    let boundary = epoch.boundary.clone();
    let rescan = ev.need_rescan();
    if rescan {
        tracing::debug!(
            target: "tola::watch",
            kind = ?ev.kind,
            info = ev.info(),
            paths = ?ev.paths,
            "filesystem watcher requested a rescan"
        );
    }
    let hook_rescan = rescan
        && !ev.paths.is_empty()
        && normalized_event_paths(&ev)
            .filter_map(|path| {
                let classified = boundary.classify_change(&path, ev.kind);
                (!classified.is_empty()).then_some(classified)
            })
            .all(ClassifiedPath::is_hook_output)
        && normalized_event_paths(&ev)
            .any(|path| !boundary.classify_change(&path, ev.kind).is_empty());
    if rescan && !hook_rescan {
        dispatch_watch_message(
            WatchMessage::Event {
                event: ev,
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
        boundary.invalidated_by(&ev)
    };
    if invalidated.is_empty() && !accepts_event(ev.kind) && !hook_rescan {
        return;
    }
    let mut read_budget = COMMITTED_CONTENT_READ_BUDGET;
    let classified = normalized_event_paths(&ev)
        .filter(|path| {
            rescan
                || !invalidated.is_empty()
                || !is_editor_temp(path)
                || boundary.admits_editor_artifact(path, is_structural(ev.kind))
        })
        .filter_map(|path| {
            let classified = if invalidated.is_empty() {
                boundary.classify_change(&path, ev.kind)
            } else {
                ClassifiedPath::new(invalidated, RebuildScope::FullSite)
            };
            if classified.is_empty() {
                return None;
            }
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
        let mut accepted = notify::Event::new(ev.kind);
        accepted.attrs = ev.attrs.clone();
        accepted.paths = classified_paths
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        let producers =
            classified_paths
                .iter()
                .fold(ProducerSet::default(), |mut acc, (_, classified)| {
                    acc.insert(classified.producers());
                    acc
                });
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

pub(super) fn dispatch_watch_message<S: WatchMessageSink>(
    mut msg: WatchMessage,
    epoch: &mut EventEpochState,
    events: &S,
) {
    match &msg {
        WatchMessage::Event {
            event,
            classified_paths,
            producers,
            invalidated,
            ..
        } => {
            let hook_outputs_only = event.need_rescan()
                && !classified_paths.is_empty()
                && classified_paths
                    .iter()
                    .all(|(_, classified)| classified.is_hook_output());
            if event.need_rescan() && !hook_outputs_only {
                epoch.advance_site_rebuild(*producers);
            } else {
                epoch.advance_classified(classified_paths.iter().cloned());
                if is_structural(event.kind) {
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
    if let WatchMessage::Event { revision, .. } = &mut msg {
        *revision = epoch.revision;
    }
    if events.send_message(msg).is_some() {
        events.mark_overflow_ready(epoch.revision);
    }
}

pub(super) fn take_rebuild_batch(
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

/// Roots excluded from development rebuilds for one resolved configuration.
#[derive(Default)]
pub(super) struct RebuildPathFilter {
    excluded: Vec<tola_input::FilesystemSourceIdentity>,
}

impl RebuildPathFilter {
    pub(super) fn internal(root: &Path) -> Self {
        use tola_build::filesystem::SITE_BUILD_LOCK_FILE;
        use tola_input::{FilesystemSourceIdentity, INTERNAL_DIR};
        Self {
            excluded: [root.join(INTERNAL_DIR), root.join(SITE_BUILD_LOCK_FILE)]
                .iter()
                .map(|path| FilesystemSourceIdentity::from_path(path))
                .collect(),
        }
    }

    pub(super) fn for_config(cfg: &SiteConfig) -> Self {
        use tola_build::filesystem::swap_workspace;
        let mut filter = Self::internal(cfg.root());
        filter.excluded.extend(
            [
                Some(cfg.build().publish_dir.clone()),
                swap_workspace(&cfg.build().publish_dir),
                cfg.vendor().path.as_deref().and_then(swap_workspace),
            ]
            .into_iter()
            .flatten()
            .map(|path| tola_input::FilesystemSourceIdentity::from_path(&path)),
        );
        filter
    }

    pub(super) fn accepts(&self, path: &Path) -> bool {
        !self.excludes_path(&tola_input::lexical_path_identity(path))
            && !self.excludes_path(&tola_input::normalize_existing_prefix(path))
    }

    fn excludes_path(&self, path: &Path) -> bool {
        self.excluded.iter().any(|boundary| {
            path.starts_with(boundary.logical_path()) || path.starts_with(boundary.physical_path())
        })
    }
}

pub(super) fn hook_output_source_identities(
    cfg: &SiteConfig,
) -> Vec<tola_input::FilesystemSourceIdentity> {
    development_before_build_hooks(cfg)
        .flat_map(|hook| hook.generates.iter())
        .map(|path| tola_input::FilesystemSourceIdentity::from_path(&cfg.root().join(path)))
        .collect()
}
