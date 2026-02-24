//! FileSystem Actor
//!
//! Watches for file changes and sends debounced events to the CompilerActor.
//! Implements the "Watcher-First" pattern for zero event loss.

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
    let Some(events) = debouncer.take_if_ready() else {
        return Ok(());
    };

    // If initial build failed, trigger retry scan on any file change
    if !crate::core::is_healthy() {
        if events.is_empty() {
            return Ok(());
        }
        let changed_paths = events.all_paths();
        crate::log!("watch"; "retrying scan after change");
        compiler_tx
            .send(CompilerMsg::RetryScan { changed_paths })
            .await
            .map_err(|_| ())?;
        return Ok(());
    }

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
    let (created, modified, removed) = events.split();

    // Classify modified files through the existing pipeline
    let result = classify_changes(&modified, config);

    if result.config_changed {
        return vec![CompilerMsg::FullRebuild];
    }

    let mut messages = Vec::new();

    if !created.is_empty() {
        messages.push(CompilerMsg::ContentCreated(created));
    }

    if !removed.is_empty() {
        messages.push(CompilerMsg::ContentRemoved(removed));
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
// Debouncer
// =============================================================================

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

    fn add_event(&mut self, event: &notify::Event) {
        use notify::EventKind;

        let kind = match event.kind {
            EventKind::Create(_) => ChangeKind::Created,
            EventKind::Remove(_) => ChangeKind::Removed,
            EventKind::Modify(_) => ChangeKind::Modified,
            _ => return,
        };

        for path in &event.paths {
            if is_temp_file(path) {
                continue;
            }

            let path = normalize_path(path);

            // First event for this path wins (later events within same batch are redundant)
            if self.changes.contains_key(&path) {
                continue;
            }

            self.changes.insert(path, kind);
            self.last_event = Some(std::time::Instant::now());
        }
    }

    /// Take events if debounce + cooldown elapsed, applying merge rules.
    ///
    /// Only sets `last_compile` when returning non-empty events.
    fn take_if_ready(&mut self) -> Option<DebouncedEvents> {
        if !self.is_ready() {
            return None;
        }

        if !crate::core::is_serving() {
            return None;
        }

        let mut changes = std::mem::take(&mut self.changes);
        self.last_event = None;

        // Existence-based corrections
        let paths: Vec<_> = changes.keys().cloned().collect();
        for path in paths {
            let kind = changes[&path];
            match kind {
                ChangeKind::Created if !path.exists() => {
                    // Created but already gone → discard
                    changes.remove(&path);
                }
                ChangeKind::Modified if !path.exists() => {
                    // Modified but gone → treat as removed
                    changes.insert(path, ChangeKind::Removed);
                }
                ChangeKind::Removed if path.exists() => {
                    // Removed but still there → treat as modified (macOS FSEvents)
                    changes.insert(path, ChangeKind::Modified);
                }
                _ => {}
            }
        }

        // Promote modified files not in AddressSpace to created
        {
            use crate::address::GLOBAL_ADDRESS_SPACE;
            let space = GLOBAL_ADDRESS_SPACE.read();
            let to_promote: Vec<_> = changes
                .iter()
                .filter(|(_, k)| **k == ChangeKind::Modified)
                .filter(|(p, _)| space.url_for_source(p).is_none())
                .map(|(p, _)| p.clone())
                .collect();
            for path in to_promote {
                changes.insert(path, ChangeKind::Created);
            }
        }

        // Filter created/modified to files only (not directories)
        changes.retain(|p, k| match k {
            ChangeKind::Created | ChangeKind::Modified => p.is_file(),
            ChangeKind::Removed => true, // can't check is_file on deleted paths
        });

        if changes.is_empty() {
            return None;
        }

        self.last_compile = Some(std::time::Instant::now());
        Some(DebouncedEvents(changes.into_iter().collect()))
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
        debouncer.add_event(&make_event(
            vec!["/tmp/a.typ", "/tmp/a.typ"],
            modify_kind(),
        ));
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
}
