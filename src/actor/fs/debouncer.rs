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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

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

    fn add_event(debouncer: &mut Debouncer, path: &str, kind: notify::EventKind) {
        debouncer.add_event(&make_event(vec![path], kind));
    }

    fn assert_change_kind(debouncer: &Debouncer, path: &str, expected: ChangeKind) {
        assert_eq!(debouncer.changes[&PathBuf::from(path)], expected);
    }

    #[test]
    fn stores_non_temp_events_by_path() {
        let mut debouncer = Debouncer::new();
        assert!(!debouncer.is_ready());

        add_event(&mut debouncer, "/tmp/a.typ", create_kind());
        add_event(&mut debouncer, "/tmp/b.typ", modify_kind());
        add_event(&mut debouncer, "/tmp/c.typ", remove_kind());

        assert_eq!(debouncer.changes.len(), 3);
        assert_change_kind(&debouncer, "/tmp/a.typ", ChangeKind::Created);
        assert_change_kind(&debouncer, "/tmp/b.typ", ChangeKind::Modified);
        assert_change_kind(&debouncer, "/tmp/c.typ", ChangeKind::Removed);
    }

    #[test]
    fn ignores_temp_files_without_refreshing_debounce_window() {
        let mut debouncer = Debouncer::new();

        add_event(&mut debouncer, "/tmp/real.typ", modify_kind());
        assert!(debouncer.last_event.is_some());
        let first_time = debouncer.last_event.unwrap();

        std::thread::sleep(Duration::from_millis(5));

        debouncer.add_event(&make_event(vec!["/tmp/.swp"], modify_kind()));
        assert_eq!(debouncer.last_event.unwrap(), first_time);
        assert_eq!(debouncer.changes.len(), 1);
    }

    #[test]
    fn first_create_or_modify_event_wins_for_same_path() {
        let mut debouncer = Debouncer::new();

        add_event(&mut debouncer, "/tmp/a.typ", create_kind());
        add_event(&mut debouncer, "/tmp/a.typ", modify_kind());

        assert_eq!(debouncer.changes.len(), 1);
        assert_change_kind(&debouncer, "/tmp/a.typ", ChangeKind::Created);
    }

    #[test]
    fn deduplicates_same_notify_event_paths() {
        let mut debouncer = Debouncer::new();

        debouncer.add_event(&make_event(vec!["/tmp/a.typ", "/tmp/a.typ"], modify_kind()));

        assert_eq!(debouncer.changes.len(), 1);
    }

    #[test]
    fn sleep_duration_without_events_is_idle() {
        let debouncer = Debouncer::new();

        assert!(debouncer.sleep_duration() >= Duration::from_secs(3600));
    }

    #[test]
    fn sleep_duration_after_event_tracks_debounce_window() {
        let mut debouncer = Debouncer::new();
        debouncer.last_event = Some(std::time::Instant::now());

        let dur = debouncer.sleep_duration();

        assert!(dur >= Duration::from_millis(DEBOUNCE_MS - 10));
        assert!(dur <= Duration::from_millis(DEBOUNCE_MS + 10));
    }

    #[test]
    fn sleep_duration_respects_rebuild_cooldown() {
        let mut debouncer = Debouncer::new();
        debouncer.last_event = Some(std::time::Instant::now());
        debouncer.last_compile = Some(std::time::Instant::now());

        let dur = debouncer.sleep_duration();

        assert!(dur >= Duration::from_millis(REBUILD_COOLDOWN_MS - 10));
        assert!(dur <= Duration::from_millis(REBUILD_COOLDOWN_MS + 10));
    }

    #[test]
    fn event_state_transitions_preserve_effective_change() {
        let mut restored = Debouncer::new();
        add_event(&mut restored, "/tmp/a.typ", remove_kind());
        assert_change_kind(&restored, "/tmp/a.typ", ChangeKind::Removed);
        add_event(&mut restored, "/tmp/a.typ", create_kind());
        assert_eq!(restored.changes.len(), 1);
        assert_change_kind(&restored, "/tmp/a.typ", ChangeKind::Created);

        let mut discarded = Debouncer::new();
        add_event(&mut discarded, "/tmp/a.typ", create_kind());
        assert_change_kind(&discarded, "/tmp/a.typ", ChangeKind::Created);
        add_event(&mut discarded, "/tmp/a.typ", remove_kind());
        assert!(
            discarded.changes.is_empty(),
            "created+removed should discard"
        );

        let mut upgraded = Debouncer::new();
        add_event(&mut upgraded, "/tmp/a.typ", modify_kind());
        add_event(&mut upgraded, "/tmp/a.typ", remove_kind());
        assert_eq!(upgraded.changes.len(), 1);
        assert_change_kind(&upgraded, "/tmp/a.typ", ChangeKind::Removed);
    }
}
