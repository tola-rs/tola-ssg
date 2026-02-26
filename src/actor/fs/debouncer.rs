use std::path::Path;
use std::time::Duration;

use rustc_hash::FxHashMap;

use super::types::ChangeKind;
use crate::utils::path::normalize_path;

pub(super) const DEBOUNCE_MS: u64 = 300;
pub(super) const REBUILD_COOLDOWN_MS: u64 = 800;

/// Pure debouncer: only handles timing and event deduplication.
/// No business logic, no global state access.
pub(super) struct Debouncer {
    /// Path → ChangeKind (dedup is free via HashMap key uniqueness)
    pub(super) changes: FxHashMap<std::path::PathBuf, ChangeKind>,
    pub(super) last_event: Option<std::time::Instant>,
    pub(super) last_compile: Option<std::time::Instant>,
}

impl Debouncer {
    pub(super) fn new() -> Self {
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
    pub(super) fn add_event(&mut self, event: &notify::Event) {
        use notify::EventKind;

        let kind = match event.kind {
            EventKind::Create(_) => ChangeKind::Created,
            EventKind::Remove(_) => ChangeKind::Removed,
            EventKind::Modify(modify) => {
                // Ignore metadata-only changes (mtime/atime/chmod noise)
                // may trigger endless rebuild loops
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
    pub(super) fn take_if_ready(&mut self) -> Option<FxHashMap<std::path::PathBuf, ChangeKind>> {
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

    pub(super) fn is_ready(&self) -> bool {
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
    pub(super) fn sleep_duration(&self) -> Duration {
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

/// Check if path is a temp/backup file (editor artifacts).
fn is_temp_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    matches!(ext, "bck" | "bak" | "backup" | "swp" | "swo" | "tmp")
        || name.ends_with('~')
        || name.starts_with('.')
}
