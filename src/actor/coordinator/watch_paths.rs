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
