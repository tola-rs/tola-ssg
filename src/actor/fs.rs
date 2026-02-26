//! FileSystem Actor
//!
//! Watches for file changes and sends debounced events to the CompilerActor.
//! Implements the "Watcher-First" pattern for zero event loss.
//!
//! Architecture:
//! ```text
//! Watcher → Debouncer (pure timing) → Classifier (business logic) → CompilerMsg
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;

use super::messages::CompilerMsg;
use crate::config::SiteConfig;
use crate::reload::classify::classify_changes;
use crate::utils::path::normalize_path;

/// Debounce configuration
const DEBOUNCE_MS: u64 = 300;
const REBUILD_COOLDOWN_MS: u64 = 800;

/// Check if path is a temp/backup file (editor artifacts)
fn is_temp_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    matches!(ext, "bck" | "bak" | "backup" | "swp" | "swo" | "tmp")
        || name.ends_with('~')
        || name.starts_with('.')
}

/// FileSystem Actor - watches for file changes
pub struct FsActor {
    /// Channel to receive notify events (sync -> async bridge)
    notify_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Watcher handle (must be kept alive)
    _watcher: RecommendedWatcher,
    /// Channel to send messages to CompilerActor
    compiler_tx: mpsc::Sender<CompilerMsg>,
    /// Debouncer state
    debouncer: Debouncer,
    /// Site configuration for file classification
    config: Arc<SiteConfig>,
}

impl FsActor {
    /// Create a new FsActor with Watcher-First pattern
    ///
    /// The watcher starts immediately, buffering events while the caller
    /// performs initial build. This eliminates the "vacuum period".
    #[rustfmt::skip]
    pub fn new(
        paths: Vec<PathBuf>,
        compiler_tx: mpsc::Sender<CompilerMsg>,
        config: Arc<SiteConfig>,
    ) -> notify::Result<Self> {
        // Create sync channel for notify (it doesn't support async)
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();

        // Create and configure watcher IMMEDIATELY
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = notify_tx.send(res);
        })?;

        // Start watching all paths (skip non-existent paths to handle race conditions)
        for path in &paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive)?;
            }
        }

        // Events are now buffering in notify_rx while caller does initial build

        Ok(Self {
            notify_rx,
            _watcher: watcher,
            compiler_tx,
            debouncer: Debouncer::new(),
            config,
        })
    }

    /// Run the actor event loop
    pub async fn run(self) {
        // Extract fields before consuming self
        let notify_rx = self.notify_rx;
        let compiler_tx = self.compiler_tx;
        let config = self.config;
        let mut debouncer = self.debouncer;

        let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<notify::Event>(64);

        // Spawn a thread to poll notify events and send to async channel
        std::thread::spawn(move || {
            while let Ok(result) = notify_rx.recv() {
                match result {
                    Ok(event) => {
                        if async_tx.blocking_send(event).is_err() {
                            break; // Receiver dropped
                        }
                    }
                    Err(e) => crate::log!("watch"; "notify error: {}", e),
                }
            }
        });

        loop {
            tokio::select! {
                biased;
                Some(event) = async_rx.recv() => debouncer.add_event(&event),
                _ = tokio::time::sleep(debouncer.sleep_duration()) => {
                    if process_changes(&mut debouncer, &compiler_tx, &config).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// Process debounced file changes
///
/// Returns `Err(())` if CompilerActor shut down
async fn process_changes(
    debouncer: &mut Debouncer,
    compiler_tx: &mpsc::Sender<CompilerMsg>,
    config: &SiteConfig,
) -> Result<(), ()> {
    // Must be serving to process events (check BEFORE taking to preserve events)
    if !crate::core::is_serving() {
        return Ok(());
    }

    // Get raw events from debouncer (pure timing)
    let Some(raw_events) = debouncer.take_if_ready() else {
        return Ok(());
    };

    // If initial build failed, trigger retry scan on any file change
    if !crate::core::is_healthy() {
        if raw_events.is_empty() {
            return Ok(());
        }
        let changed_paths: Vec<_> = raw_events.keys().cloned().collect();
        crate::debug!("watch"; "retrying scan after change");
        compiler_tx
            .send(CompilerMsg::RetryScan { changed_paths })
            .await
            .map_err(|_| ())?;
        return Ok(());
    }

    // Classify events (business logic)
    let Some(events) = EventClassifier::classify(raw_events, config) else {
        return Ok(());
    };

    log_events(&events);

    let messages = events_to_messages(events, config);
    if messages.is_empty() {
        return Ok(());
    }

    for msg in messages {
        compiler_tx.send(msg).await.map_err(|_| ())?;
    }

    Ok(())
}

/// Log file change events
fn log_events(events: &DebouncedEvents) {
    for (path, kind) in &events.0 {
        crate::debug!("watch"; "{}: {}", kind.label(), path.display());
    }
}

/// Convert DebouncedEvents to CompilerMsg(s)
fn events_to_messages(events: DebouncedEvents, config: &SiteConfig) -> Vec<CompilerMsg> {
    use crate::reload::classify::{FileCategory, categorize_path};

    let (created, modified, removed) = events.split();

    // Classify modified files through the existing pipeline
    let result = classify_changes(&modified, config);

    if result.config_changed {
        return vec![CompilerMsg::FullRebuild];
    }

    let mut messages = Vec::new();

    // Process removed files FIRST (before created) to handle renames correctly
    // This ensures old state is cleaned up before new files are compiled
    let removed_content: Vec<_> = removed
        .into_iter()
        .filter(|p| matches!(categorize_path(p, config), FileCategory::Content(_)))
        .collect();
    if !removed_content.is_empty() {
        messages.push(CompilerMsg::ContentRemoved(removed_content));
    }

    // Filter created files to content files in content directory
    let created_content: Vec<_> = created
        .into_iter()
        .filter(|p| matches!(categorize_path(p, config), FileCategory::Content(_)))
        .collect();
    if !created_content.is_empty() {
        messages.push(CompilerMsg::ContentCreated(created_content));
    }

    if !result.asset_changed.is_empty() {
        messages.push(CompilerMsg::AssetChange(result.asset_changed));
    }

    if !result.compile_queue.is_empty() {
        let changed_paths: Vec<PathBuf> =
            result.classified.iter().map(|(p, _)| p.clone()).collect();
        messages.push(CompilerMsg::Compile {
            queue: result.compile_queue,
            changed_paths,
        });
    }

    messages
}

// =============================================================================
// Change types
// =============================================================================

/// What happened to a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    Created,
    Modified,
    Removed,
}

impl ChangeKind {
    fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }
}

/// Debounced file events, categorized by type
pub(crate) struct DebouncedEvents(Vec<(PathBuf, ChangeKind)>);

impl DebouncedEvents {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn all_paths(&self) -> Vec<PathBuf> {
        self.0.iter().map(|(p, _)| p.clone()).collect()
    }

    /// Split into (created, modified, removed) path lists
    fn split(self) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
        let mut created = Vec::new();
        let mut modified = Vec::new();
        let mut removed = Vec::new();
        for (path, kind) in self.0 {
            match kind {
                ChangeKind::Created => created.push(path),
                ChangeKind::Modified => modified.push(path),
                ChangeKind::Removed => removed.push(path),
            }
        }
        (created, modified, removed)
    }
}

// =============================================================================
// Debouncer - Pure timing and event deduplication
// =============================================================================

/// Pure debouncer: only handles timing and event deduplication.
/// No business logic, no global state access.
struct Debouncer {
    /// Path → ChangeKind (dedup is free via HashMap key uniqueness)
    changes: FxHashMap<PathBuf, ChangeKind>,
    last_event: Option<std::time::Instant>,
    last_compile: Option<std::time::Instant>,
}

impl Debouncer {
    fn new() -> Self {
        Self {
            changes: FxHashMap::default(),
            last_event: None,
            last_compile: None,
        }
    }

    /// Add a notify event, applying dedup rules:
    /// - Remove + Create/Modify → Create/Modify (file was restored)
    /// - Create/Modify + Remove → Remove (file was deleted)
    /// - Same type events: first event wins
    fn add_event(&mut self, event: &notify::Event) {
        use notify::EventKind;

        let kind = match event.kind {
            EventKind::Create(_) => ChangeKind::Created,
            EventKind::Remove(_) => ChangeKind::Removed,
            EventKind::Modify(modify) => {
                // Ignore metadata-only changes (mtime/atime/chmod noise)
                // maybe trigger endless rebuild loops
                if matches!(modify, notify::event::ModifyKind::Metadata(_)) {
                    return;
                }
                ChangeKind::Modified
            }
            _ => return,
        };

        crate::debug!("watch"; "raw notify: {:?} {:?}", event.kind, event.paths);

        for path in &event.paths {
            if is_temp_file(path) {
                continue;
            }

            let path = normalize_path(path);

            if let Some(&existing) = self.changes.get(&path) {
                // State transitions:
                // - Removed -> Created/Modified: restored, use new event
                // - Modified -> Removed: deleted, upgrade to Removed
                // - Created -> Removed: appeared then vanished, discard (no-op)
                // - otherwise: first event wins
                match (existing, kind) {
                    (ChangeKind::Removed, ChangeKind::Created | ChangeKind::Modified) => {
                        // File was deleted then restored → use the restore event
                        crate::debug!("watch"; "restore {}->created: {}", existing.label(), path.display());
                        self.changes.insert(path, kind);
                    }
                    (ChangeKind::Modified, ChangeKind::Removed) => {
                        // Tracked file was modified then deleted → upgrade to Removed
                        crate::debug!("watch"; "upgrade modified->removed: {}", path.display());
                        self.changes.insert(path, ChangeKind::Removed);
                    }
                    (ChangeKind::Created, ChangeKind::Removed) => {
                        // New file appeared then vanished within window → no-op
                        crate::debug!("watch"; "discard created+removed: {}", path.display());
                        self.changes.remove(&path);
                    }
                    _ => {
                        // Same kind or other combos (Created+Modified, etc.) → first wins
                        continue;
                    }
                }
                self.last_event = Some(std::time::Instant::now());
                continue;
            }

            crate::debug!("watch"; "event {}: {}", kind.label(), path.display());
            self.changes.insert(path, kind);
            self.last_event = Some(std::time::Instant::now());
        }
    }

    /// Take raw events if debounce + cooldown elapsed.
    /// Returns raw events without any business logic applied.
    fn take_if_ready(&mut self) -> Option<FxHashMap<PathBuf, ChangeKind>> {
        if !self.is_ready() {
            return None;
        }

        let changes = std::mem::take(&mut self.changes);
        self.last_event = None;

        if changes.is_empty() {
            return None;
        }

        self.last_compile = Some(std::time::Instant::now());
        Some(changes)
    }

    fn is_ready(&self) -> bool {
        let Some(last_event) = self.last_event else {
            return false;
        };

        if last_event.elapsed() < Duration::from_millis(DEBOUNCE_MS) {
            return false;
        }

        if let Some(last_compile) = self.last_compile
            && last_compile.elapsed() < Duration::from_millis(REBUILD_COOLDOWN_MS)
        {
            return false;
        }

        !self.changes.is_empty()
    }

    /// Precise sleep duration until next possible ready time.
    fn sleep_duration(&self) -> Duration {
        let Some(last_event) = self.last_event else {
            return Duration::from_secs(86400);
        };

        let debounce_remaining =
            Duration::from_millis(DEBOUNCE_MS).saturating_sub(last_event.elapsed());

        let cooldown_remaining = self
            .last_compile
            .map(|t| Duration::from_millis(REBUILD_COOLDOWN_MS).saturating_sub(t.elapsed()))
            .unwrap_or(Duration::ZERO);

        debounce_remaining
            .max(cooldown_remaining)
            .max(Duration::from_millis(1))
    }
}

// =============================================================================
// EventClassifier - Business logic for event classification
// =============================================================================

/// Classifies raw debounced events into final DebouncedEvents.
///
/// Pipeline: correct_by_existence → recover_from_dir_events → promote_untracked → filter_actionable
struct EventClassifier;

impl EventClassifier {
    /// Main classification pipeline.
    fn classify(
        raw: FxHashMap<PathBuf, ChangeKind>,
        config: &SiteConfig,
    ) -> Option<DebouncedEvents> {
        let mut changes = raw;

        Self::correct_by_existence(&mut changes);
        Self::recover_from_dir_events(&mut changes);
        Self::promote_untracked(&mut changes, config);
        Self::filter_actionable(&mut changes);

        if changes.is_empty() {
            return None;
        }
        Some(DebouncedEvents(changes.into_iter().collect()))
    }

    /// Reconcile event kinds with actual filesystem state.
    ///
    /// The watcher may report stale events (e.g., Created for a file that's already
    /// been deleted, or Removed for a file that still exists after an atomic save).
    fn correct_by_existence(changes: &mut FxHashMap<PathBuf, ChangeKind>) {
        let paths: Vec<_> = changes.keys().cloned().collect();
        for path in paths {
            let kind = changes[&path];
            let exists = path.exists();
            match kind {
                ChangeKind::Created if !exists => {
                    crate::debug!("watch"; "discard created (gone): {}", path.display());
                    changes.remove(&path);
                }
                ChangeKind::Modified if !exists => {
                    crate::debug!("watch"; "upgrade modified->removed: {}", path.display());
                    changes.insert(path, ChangeKind::Removed);
                }
                ChangeKind::Removed if exists => {
                    crate::debug!("watch"; "downgrade removed->modified: {}", path.display());
                    changes.insert(path, ChangeKind::Modified);
                }
                _ => {}
            }
        }
    }

    /// Recover file-level events from directory-level events.
    ///
    /// Both kqueue and FSEvents may fail to deliver file-level events after a file
    /// is deleted and recreated (different inode/fd). We only get a directory Modify
    /// event. Scan modified directories to detect:
    /// - Tracked files that disappeared → Removed
    /// - Untracked files that appeared  → Created
    fn recover_from_dir_events(changes: &mut FxHashMap<PathBuf, ChangeKind>) {
        use crate::address::GLOBAL_ADDRESS_SPACE;

        let modified_dirs: Vec<PathBuf> = changes
            .iter()
            .filter(|(_, k)| **k == ChangeKind::Modified)
            .filter(|(p, _)| p.is_dir())
            .map(|(p, _)| p.clone())
            .collect();

        if modified_dirs.is_empty() {
            return;
        }

        let space = GLOBAL_ADDRESS_SPACE.read();

        for dir in &modified_dirs {
            Self::detect_disappeared(&space, dir, changes);
            Self::detect_appeared(&space, dir, changes);
        }
    }

    /// Detect tracked files that no longer exist in a directory.
    fn detect_disappeared(
        space: &crate::address::AddressSpace,
        dir: &Path,
        changes: &mut FxHashMap<PathBuf, ChangeKind>,
    ) {
        for source in space.iter_sources() {
            if source.parent() == Some(dir) && !source.exists() && !changes.contains_key(source) {
                crate::debug!("watch"; "dir-scan found missing: {}", source.display());
                changes.insert(source.to_path_buf(), ChangeKind::Removed);
            }
        }
    }

    /// Detect new files that exist in a directory but aren't tracked.
    fn detect_appeared(
        space: &crate::address::AddressSpace,
        dir: &Path,
        changes: &mut FxHashMap<PathBuf, ChangeKind>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = normalize_path(&entry.path());
            if path.is_file()
                && !changes.contains_key(&path)
                && space.url_for_source(&path).is_none()
            {
                crate::debug!("watch"; "dir-scan found untracked: {}", path.display());
                changes.insert(path, ChangeKind::Created);
            }
        }
    }

    /// Promote Modified files not in AddressSpace to Created.
    ///
    /// A file that's modified but not yet tracked is effectively a new file
    /// (e.g., created by an editor that writes-then-renames).
    ///
    /// IMPORTANT: Only promote content files. Deps files (templates, utils) and
    /// Asset files should remain as Modified so they can be processed correctly
    /// by classify_changes().
    fn promote_untracked(changes: &mut FxHashMap<PathBuf, ChangeKind>, config: &SiteConfig) {
        use crate::address::GLOBAL_ADDRESS_SPACE;
        use crate::reload::classify::{FileCategory, categorize_path};

        let space = GLOBAL_ADDRESS_SPACE.read();
        let to_promote: Vec<_> = changes
            .iter()
            .filter(|(_, k)| **k == ChangeKind::Modified)
            .filter(|(p, _)| space.url_for_source(p).is_none())
            // Don't promote deps/asset files - they need to stay Modified for proper handling
            .filter(|(p, _)| {
                !matches!(
                    categorize_path(p, config),
                    FileCategory::Deps | FileCategory::Asset
                )
            })
            .map(|(p, _)| p.clone())
            .collect();
        for path in to_promote {
            changes.insert(path, ChangeKind::Created);
        }
    }

    /// Filter to actionable events only.
    ///
    /// - Created/Modified: must be a file (not a directory)
    /// - Removed: must still be tracked in AddressSpace (prevents duplicate removals)
    fn filter_actionable(changes: &mut FxHashMap<PathBuf, ChangeKind>) {
        use crate::address::GLOBAL_ADDRESS_SPACE;

        let space = GLOBAL_ADDRESS_SPACE.read();
        changes.retain(|p, k| match k {
            ChangeKind::Created | ChangeKind::Modified => p.is_file(),
            ChangeKind::Removed => {
                let tracked = space.url_for_source(p).is_some();
                if !tracked {
                    crate::debug!("watch"; "filter removed (not tracked): {}", p.display());
                }
                tracked
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(paths: Vec<&str>, kind: notify::EventKind) -> notify::Event {
        notify::Event {
            kind,
            paths: paths.into_iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    fn modify_kind() -> notify::EventKind {
        notify::EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Any,
        ))
    }

    fn create_kind() -> notify::EventKind {
        notify::EventKind::Create(notify::event::CreateKind::File)
    }

    fn remove_kind() -> notify::EventKind {
        notify::EventKind::Remove(notify::event::RemoveKind::File)
    }

    #[test]
    fn test_debouncer_empty() {
        let debouncer = Debouncer::new();
        assert!(!debouncer.is_ready());
    }

    #[test]
    fn test_event_routing_by_kind() {
        let mut debouncer = Debouncer::new();

        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
        debouncer.add_event(&make_event(vec!["/tmp/b.typ"], modify_kind()));
        debouncer.add_event(&make_event(vec!["/tmp/c.typ"], remove_kind()));

        assert_eq!(debouncer.changes.len(), 3);
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/a.typ")],
            ChangeKind::Created
        );
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/b.typ")],
            ChangeKind::Modified
        );
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/c.typ")],
            ChangeKind::Removed
        );
    }

    #[test]
    fn test_temp_file_ignored() {
        let mut debouncer = Debouncer::new();

        debouncer.add_event(&make_event(vec!["/tmp/real.typ"], modify_kind()));
        assert!(debouncer.last_event.is_some());
        let first_time = debouncer.last_event.unwrap();

        std::thread::sleep(Duration::from_millis(5));

        // Temp file event — should NOT update last_event or add to changes
        debouncer.add_event(&make_event(vec!["/tmp/.swp"], modify_kind()));
        assert_eq!(debouncer.last_event.unwrap(), first_time);
        assert_eq!(debouncer.changes.len(), 1);
    }

    #[test]
    fn test_dedup_first_event_wins() {
        let mut debouncer = Debouncer::new();

        // Same path: create then modify — first one (create) wins
        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], modify_kind()));

        assert_eq!(debouncer.changes.len(), 1);
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/a.typ")],
            ChangeKind::Created
        );
    }

    #[test]
    fn test_dedup_same_event() {
        let mut debouncer = Debouncer::new();
        debouncer.add_event(&make_event(vec!["/tmp/a.typ", "/tmp/a.typ"], modify_kind()));
        assert_eq!(debouncer.changes.len(), 1);
    }

    #[test]
    fn test_sleep_duration_no_events() {
        let debouncer = Debouncer::new();
        assert!(debouncer.sleep_duration() >= Duration::from_secs(3600));
    }

    #[test]
    fn test_sleep_duration_after_event() {
        let mut debouncer = Debouncer::new();
        debouncer.last_event = Some(std::time::Instant::now());

        let dur = debouncer.sleep_duration();
        assert!(dur >= Duration::from_millis(DEBOUNCE_MS - 10));
        assert!(dur <= Duration::from_millis(DEBOUNCE_MS + 10));
    }

    #[test]
    fn test_sleep_duration_respects_cooldown() {
        let mut debouncer = Debouncer::new();
        debouncer.last_event = Some(std::time::Instant::now());
        debouncer.last_compile = Some(std::time::Instant::now());

        let dur = debouncer.sleep_duration();
        assert!(dur >= Duration::from_millis(REBUILD_COOLDOWN_MS - 10));
        assert!(dur <= Duration::from_millis(REBUILD_COOLDOWN_MS + 10));
    }

    #[test]
    fn test_debounced_events_split() {
        let events = DebouncedEvents(vec![
            (PathBuf::from("/a.typ"), ChangeKind::Created),
            (PathBuf::from("/b.typ"), ChangeKind::Modified),
            (PathBuf::from("/c.typ"), ChangeKind::Removed),
            (PathBuf::from("/d.typ"), ChangeKind::Created),
        ]);

        let (created, modified, removed) = events.split();
        assert_eq!(created.len(), 2);
        assert_eq!(modified.len(), 1);
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn test_remove_then_create_restores() {
        let mut debouncer = Debouncer::new();

        // File removed, then restored (created) — should become Created
        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], remove_kind()));
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/a.typ")],
            ChangeKind::Removed
        );

        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
        assert_eq!(debouncer.changes.len(), 1);
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/a.typ")],
            ChangeKind::Created
        );
    }

    #[test]
    fn test_create_then_remove_discards() {
        let mut debouncer = Debouncer::new();

        // File created, then removed — net no-op, should be discarded entirely
        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], create_kind()));
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/a.typ")],
            ChangeKind::Created
        );

        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], remove_kind()));
        assert!(
            debouncer.changes.is_empty(),
            "created+removed should discard"
        );
    }

    #[test]
    fn test_modify_then_remove_upgrades() {
        let mut debouncer = Debouncer::new();

        // File modified, then removed — should upgrade to Removed
        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], modify_kind()));
        debouncer.add_event(&make_event(vec!["/tmp/a.typ"], remove_kind()));
        assert_eq!(debouncer.changes.len(), 1);
        assert_eq!(
            debouncer.changes[&PathBuf::from("/tmp/a.typ")],
            ChangeKind::Removed
        );
    }
}
