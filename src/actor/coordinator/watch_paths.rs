use std::path::PathBuf;

use crate::config::SiteConfig;

pub(super) fn collect_watch_paths(config: &SiteConfig) -> Vec<PathBuf> {
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
        paths.push(output_dir);
    }

    paths
}
