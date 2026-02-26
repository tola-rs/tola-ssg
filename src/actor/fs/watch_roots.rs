use std::path::PathBuf;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rustc_hash::FxHashSet;

/// Watch-root consistency manager.
///
/// Responsibility:
/// - Attach existing roots at startup
/// - Re-attach roots that were removed and recreated
pub(super) struct WatchRoots {
    desired: Vec<PathBuf>,
    attached: FxHashSet<PathBuf>,
}

impl WatchRoots {
    pub(super) fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            desired: paths,
            attached: FxHashSet::default(),
        }
    }

    pub(super) fn attach_existing(
        &mut self,
        watcher: &mut RecommendedWatcher,
    ) -> notify::Result<()> {
        for path in &self.desired {
            if !path.exists() {
                continue;
            }
            watcher.watch(path, RecursiveMode::Recursive)?;
            self.attached.insert(path.clone());
        }

        Ok(())
    }

    pub(super) fn maintain(&mut self, watcher: &mut RecommendedWatcher) {
        // Drop stale handles for roots that no longer exist.
        self.attached.retain(|path| path.exists());

        for path in &self.desired {
            if self.attached.contains(path) || !path.exists() {
                continue;
            }

            if watcher.watch(path, RecursiveMode::Recursive).is_ok() {
                self.attached.insert(path.clone());
                crate::debug!("watch"; "re-attached watch: {}", path.display());
            }
        }
    }
}
