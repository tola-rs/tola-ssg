use std::path::PathBuf;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rustc_hash::FxHashSet;

use super::is_transient_not_found;
use crate::config::SiteConfig;
use crate::logger;

/// Watched root set.
///
/// Responsibility:
/// - Attach existing roots at startup
/// - Re-attach roots that were removed and recreated
/// - Track root changes after config reload
pub(super) struct RootSet {
    desired: Vec<WatchRoot>,
    attached: FxHashSet<WatchRoot>,
}

impl RootSet {
    fn new(paths: Vec<WatchRoot>) -> Self {
        Self {
            desired: paths,
            attached: FxHashSet::default(),
        }
    }

    pub(super) fn from_config(config: &SiteConfig) -> Self {
        Self::new(collect_roots(config))
    }

    pub(super) fn attach_existing(
        &mut self,
        watcher: &mut RecommendedWatcher,
    ) -> notify::Result<()> {
        for root in &self.desired {
            if !root.path.exists() {
                continue;
            }
            match watcher.watch(&root.path, root.mode) {
                Ok(()) => {
                    self.attached.insert(root.clone());
                }
                Err(err) => {
                    let _ = watcher.unwatch(&root.path);
                    // Race-safe startup:
                    // - root may disappear between `exists()` and `watch()` during `serve --clean`
                    // - recursive watch may hit transient missing descendants (e.g. .git/objects/pack)
                    // Don't fail actor startup for single-path watch errors.
                    // maintain() will keep trying to re-attach roots.
                    let transient = !root.path.exists() || is_transient_not_found(&err);
                    if transient {
                        logger::debug(
                            "watch",
                            format_args!(
                                "skip transient watch attach error on startup: {} ({})",
                                root.path.display(),
                                err
                            ),
                        );
                    } else {
                        logger::debug(
                            "watch",
                            format_args!(
                                "skip non-transient watch attach error on startup: {} ({})",
                                root.path.display(),
                                err
                            ),
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
        self.attached.retain(|root| root.path.exists());

        for root in &self.desired {
            if self.attached.contains(root) || !root.path.exists() {
                continue;
            }

            if watcher.watch(&root.path, root.mode).is_ok() {
                self.attached.insert(root.clone());
                logger::debug(
                    "watch",
                    format_args!("re-attached watch: {}", root.path.display()),
                );
            } else {
                let _ = watcher.unwatch(&root.path);
            }
        }
    }

    pub(super) fn sync_config(&mut self, watcher: &mut RecommendedWatcher, config: &SiteConfig) {
        let desired = collect_roots(config);
        let desired_set: FxHashSet<WatchRoot> = desired.iter().cloned().collect();
        let stale: Vec<WatchRoot> = self
            .attached
            .iter()
            .filter(|root| !desired_set.contains(*root))
            .cloned()
            .collect();

        for root in stale {
            if watcher.unwatch(&root.path).is_ok() {
                logger::debug(
                    "watch",
                    format_args!("detached watch: {}", root.path.display()),
                );
            }
            self.attached.remove(&root);
        }

        self.desired = desired;
        self.maintain(watcher);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WatchRoot {
    path: PathBuf,
    mode: RecursiveMode,
}

impl WatchRoot {
    fn recursive(path: PathBuf) -> Self {
        Self {
            path,
            mode: RecursiveMode::Recursive,
        }
    }

    fn non_recursive(path: PathBuf) -> Self {
        Self {
            path,
            mode: RecursiveMode::NonRecursive,
        }
    }

    fn recursively_covers(&self, other: &Self) -> bool {
        self.mode == RecursiveMode::Recursive && other.path.starts_with(&self.path)
    }
}

fn collect_roots(config: &SiteConfig) -> Vec<WatchRoot> {
    let root = config.get_root();
    let mut roots = vec![WatchRoot::recursive(root.join(&config.build.content))];
    for dep in &config.build.deps {
        roots.push(WatchRoot::recursive(root.join(dep)));
    }

    for source in config.build.assets.nested_sources() {
        if source.exists() {
            roots.push(WatchRoot::recursive(source.to_path_buf()));
        }
    }

    for source in config.build.assets.flatten_sources() {
        if let Some(parent) = source.parent() {
            let parent_buf = parent.to_path_buf();
            if parent.exists() {
                roots.push(WatchRoot::recursive(parent_buf));
            }
        }
    }

    if config.build.css.atomic.enable {
        if config.build.css.atomic.source.is_none() && root.exists() {
            roots.push(WatchRoot::non_recursive(root.to_path_buf()));
        }
        roots.extend(
            crate::css::source::roots(config)
                .into_iter()
                .map(WatchRoot::recursive),
        );

        if let Some(config_path) = &config.build.css.atomic.config {
            let config_path = root.join(config_path);
            if config_path.exists() {
                roots.push(WatchRoot::recursive(config_path));
            }
        }
    }

    collect_hook_watch_paths(config, &mut roots);

    if config.config_path.exists() {
        roots.push(WatchRoot::recursive(config.config_path.clone()));
    }

    let output_dir = config.paths().output_dir();
    let _ = std::fs::create_dir_all(&output_dir);
    roots.push(WatchRoot::recursive(output_dir));

    dedupe_roots(&mut roots);
    roots
}

fn collect_hook_watch_paths(config: &SiteConfig, roots: &mut Vec<WatchRoot>) {
    for hook in config
        .build
        .hooks
        .pre
        .iter()
        .chain(config.build.hooks.post.iter())
    {
        for pattern in hook.watch.path_patterns() {
            if let Some(path) = hook_watch_root(config, pattern) {
                roots.push(WatchRoot::recursive(path));
            }
        }
    }
}

fn hook_watch_root(config: &SiteConfig, pattern: &str) -> Option<PathBuf> {
    let pattern = pattern
        .trim()
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/');
    if pattern.is_empty() {
        return None;
    }

    let root = config.get_root();
    let path = root.join(pattern);
    if path.exists() {
        return Some(path);
    }

    let parent = path.parent()?;
    (parent != root && parent.exists()).then(|| parent.to_path_buf())
}

fn dedupe_roots(roots: &mut Vec<WatchRoot>) {
    let mut kept: Vec<WatchRoot> = Vec::new();
    for root in roots.drain(..) {
        if let Some(existing) = kept.iter_mut().find(|existing| existing.path == root.path) {
            if root.mode == RecursiveMode::Recursive {
                existing.mode = RecursiveMode::Recursive;
            }
            continue;
        }
        if kept.iter().any(|parent| parent.recursively_covers(&root)) {
            continue;
        }
        if root.mode == RecursiveMode::Recursive {
            kept.retain(|child| !root.recursively_covers(child));
        }
        kept.push(root);
    }
    *roots = kept;
}

#[cfg(test)]
mod tests {
    use super::{WatchRoot, collect_roots, dedupe_roots};
    use crate::config::SiteConfig;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn keeps_output_root_and_drops_descendants() {
        let output = PathBuf::from("/site/public/blog");
        let mut roots = vec![
            WatchRoot::recursive(PathBuf::from("/site/content")),
            WatchRoot::recursive(output.join("showcase")),
            WatchRoot::recursive(output.clone()),
            WatchRoot::recursive(output.join("showcase/virtual-packages")),
            WatchRoot::recursive(PathBuf::from("/site/templates")),
        ];

        dedupe_roots(&mut roots);

        assert!(roots.contains(&WatchRoot::recursive(PathBuf::from("/site/content"))));
        assert!(roots.contains(&WatchRoot::recursive(output)));
        assert!(roots.contains(&WatchRoot::recursive(PathBuf::from("/site/templates"))));
        assert!(!roots.contains(&WatchRoot::recursive(PathBuf::from(
            "/site/public/blog/showcase"
        ))));
        assert!(!roots.contains(&WatchRoot::recursive(PathBuf::from(
            "/site/public/blog/showcase/virtual-packages"
        ))));
    }

    #[test]
    fn drops_redundant_descendant_watch_roots() {
        let mut roots = vec![
            WatchRoot::recursive(PathBuf::from("/site/content/posts")),
            WatchRoot::recursive(PathBuf::from("/site/assets/images")),
            WatchRoot::recursive(PathBuf::from("/site/content")),
            WatchRoot::recursive(PathBuf::from("/site/assets")),
            WatchRoot::recursive(PathBuf::from("/site/assets")),
        ];

        dedupe_roots(&mut roots);

        assert!(roots.contains(&WatchRoot::recursive(PathBuf::from("/site/content"))));
        assert!(roots.contains(&WatchRoot::recursive(PathBuf::from("/site/assets"))));
        assert!(!roots.contains(&WatchRoot::recursive(PathBuf::from("/site/content/posts"))));
        assert!(!roots.contains(&WatchRoot::recursive(PathBuf::from("/site/assets/images"))));
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn keeps_non_recursive_root_with_recursive_children() {
        let root = PathBuf::from("/site");
        let mut roots = vec![
            WatchRoot::non_recursive(root.clone()),
            WatchRoot::recursive(root.join("content")),
            WatchRoot::recursive(root.join("components")),
        ];

        dedupe_roots(&mut roots);

        assert!(roots.contains(&WatchRoot::non_recursive(root)));
        assert!(roots.contains(&WatchRoot::recursive(PathBuf::from("/site/content"))));
        assert!(roots.contains(&WatchRoot::recursive(PathBuf::from("/site/components"))));
    }

    #[test]
    fn prunes_atomic_css_auto_scan_watch_roots_and_keeps_config() {
        let temp = TempDir::new().unwrap();
        let root = crate::utils::path::normalize_path(temp.path());
        let content = root.join("content");
        let output = root.join("public");
        let tests = root.join("tests");
        let ignored = root.join("ignored");
        let atomic_config = root.join("atomic.css.toml");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&tests).unwrap();
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(&atomic_config, "").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.build.content = content;
        config.build.output = output;
        config.build.css.atomic.enable = true;
        config.build.css.atomic.config = Some(atomic_config.clone());

        let roots = collect_roots(&config);

        assert!(roots.contains(&WatchRoot::non_recursive(root.clone())));
        assert!(!roots.contains(&WatchRoot::recursive(root)));
        assert!(
            roots.contains(&WatchRoot::recursive(crate::utils::path::normalize_path(
                &tests
            )))
        );
        assert!(
            !roots.contains(&WatchRoot::recursive(crate::utils::path::normalize_path(
                &ignored
            )))
        );
        assert!(roots.contains(&WatchRoot::recursive(atomic_config)));
    }

    #[test]
    fn includes_hook_watch_patterns() {
        use crate::config::section::build::{HookConfig, WatchMode};

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let output = root.join("public");
        let src = root.join("src");
        let input = src.join("tailwind.css");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(&input, "@import 'tailwindcss';").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.content = content;
        config.build.output = output;
        config.build.hooks.pre.push(HookConfig {
            command: vec!["tailwindcss".into()],
            watch: WatchMode::Patterns(vec!["src/tailwind.css".into()]),
            ..HookConfig::default()
        });

        let roots = collect_roots(&config);

        assert!(roots.contains(&WatchRoot::recursive(input)));
    }
}
