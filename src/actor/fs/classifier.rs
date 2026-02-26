use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use super::types::{ChangeKind, DebouncedEvents};
use crate::config::SiteConfig;
use crate::utils::path::normalize_path;

/// Classifies raw debounced events into final DebouncedEvents.
///
/// Pipeline: correct_by_existence → recover_from_dir_events → promote_untracked → filter_actionable
pub(super) struct EventClassifier;

impl EventClassifier {
    /// Main classification pipeline.
    pub(super) fn classify(
        raw: FxHashMap<PathBuf, ChangeKind>,
        config: &SiteConfig,
    ) -> Option<DebouncedEvents> {
        let mut changes = raw;

        Self::correct_by_existence(&mut changes);
        Self::recover_from_dir_events(&mut changes);
        Self::promote_untracked(&mut changes, config);
        Self::filter_actionable(&mut changes, config);

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
    pub(super) fn filter_actionable(
        changes: &mut FxHashMap<PathBuf, ChangeKind>,
        config: &SiteConfig,
    ) {
        use crate::address::GLOBAL_ADDRESS_SPACE;
        use crate::reload::classify::{FileCategory, categorize_path};

        let space = GLOBAL_ADDRESS_SPACE.read();
        changes.retain(|p, k| match k {
            ChangeKind::Created | ChangeKind::Modified => p.is_file(),
            ChangeKind::Removed => {
                if matches!(categorize_path(p, config), FileCategory::Output) {
                    return true;
                }
                let tracked = space.url_for_source(p).is_some();
                if !tracked {
                    crate::debug!("watch"; "filter removed (not tracked): {}", p.display());
                }
                tracked
            }
        });
    }
}
