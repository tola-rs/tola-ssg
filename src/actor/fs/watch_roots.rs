use std::path::PathBuf;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rustc_hash::FxHashSet;

use super::is_transient_not_found;
use crate::config::SiteConfig;

/// Watch-root consistency manager.
///
/// Responsibility:
/// - Attach existing roots at startup
/// - Re-attach roots that were removed and recreated
/// - Track root changes after config reload
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

    pub(super) fn from_config(config: &SiteConfig) -> Self {
        Self::new(collect_watch_paths(config))
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

    pub(super) fn sync_config(&mut self, watcher: &mut RecommendedWatcher, config: &SiteConfig) {
        let desired = collect_watch_paths(config);
        let desired_set: FxHashSet<PathBuf> = desired.iter().cloned().collect();
        let stale: Vec<PathBuf> = self
            .attached
            .iter()
            .filter(|path| !desired_set.contains(*path))
            .cloned()
            .collect();

        for path in stale {
            if watcher.unwatch(&path).is_ok() {
                crate::debug!("watch"; "detached watch: {}", path.display());
            }
            self.attached.remove(&path);
        }

        self.desired = desired;
        self.maintain(watcher);
    }
}

fn collect_watch_paths(config: &SiteConfig) -> Vec<PathBuf> {
    let root = config.get_root();
    let mut paths = vec![root.join(&config.build.content)];
    for dep in &config.build.deps {
        paths.push(root.join(dep));
    }

    for source in config.build.assets.nested_sources() {
        if source.exists() {
            paths.push(source.to_path_buf());
        }
    }

    for source in config.build.assets.flatten_sources() {
        if let Some(parent) = source.parent() {
            let parent_buf = parent.to_path_buf();
            if parent.exists() && !paths.contains(&parent_buf) {
                paths.push(parent_buf);
            }
        }
    }

    if config.config_path.exists() {
        paths.push(config.config_path.clone());
    }

    let output_dir = config.paths().output_dir();
    let _ = std::fs::create_dir_all(&output_dir);
    if !paths.contains(&output_dir) {
        paths.push(output_dir.clone());
    }

    dedupe_output_children(&mut paths, &output_dir);

    paths
}

/// Keep output root watch, drop redundant descendants under output.
///
/// We only need to watch output root recursively; watching its children is
/// redundant and can introduce startup races during `serve --clean`.
fn dedupe_output_children(paths: &mut Vec<PathBuf>, output_root: &std::path::Path) {
    paths.retain(|path| path.as_path() == output_root || !path.starts_with(output_root));
}

#[cfg(test)]
mod tests {
    use super::dedupe_output_children;
    use std::path::PathBuf;

    #[test]
    fn keeps_output_root_and_drops_descendants() {
        let output = PathBuf::from("/site/public/blog");
        let mut paths = vec![
            PathBuf::from("/site/content"),
            output.clone(),
            output.join("showcase"),
            output.join("showcase/virtual-packages"),
            PathBuf::from("/site/templates"),
        ];

        dedupe_output_children(&mut paths, &output);

        assert!(paths.contains(&PathBuf::from("/site/content")));
        assert!(paths.contains(&output));
        assert!(paths.contains(&PathBuf::from("/site/templates")));
        assert!(!paths.contains(&PathBuf::from("/site/public/blog/showcase")));
        assert!(!paths.contains(&PathBuf::from(
            "/site/public/blog/showcase/virtual-packages"
        )));
    }
}
