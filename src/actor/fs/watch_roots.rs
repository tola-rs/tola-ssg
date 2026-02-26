use std::path::PathBuf;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rustc_hash::FxHashSet;

use super::is_transient_not_found;

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
            match watcher.watch(path, RecursiveMode::Recursive) {
                Ok(()) => {
                    self.attached.insert(path.clone());
                }
                Err(err) => {
                    // Race-safe startup:
                    // - root may disappear between `exists()` and `watch()` during `serve --clean`
                    // - recursive watch may hit transient missing descendants (e.g. .git/objects/pack)
                    // Don't fail actor startup for single-path watch errors.
                    // maintain() will keep trying to re-attach roots.
                    let transient = !path.exists() || is_transient_not_found(&err);
                    if transient {
                        crate::debug!(
                            "watch";
                            "skip transient watch attach error on startup: {} ({})",
                            path.display(),
                            err
                        );
                    } else {
                        crate::debug!(
                            "watch";
                            "skip non-transient watch attach error on startup: {} ({})",
                            path.display(),
                            err
                        );
                    }
                    continue;
                }
            }
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
