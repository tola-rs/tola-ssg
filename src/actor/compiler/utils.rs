use std::path::{Path, PathBuf};

use crate::config::SiteConfig;

pub(super) fn process_assets(paths: &[PathBuf], config: &SiteConfig) -> Vec<(PathBuf, String)> {
    use crate::asset::{process_asset, process_rel_asset};

    paths
        .iter()
        .filter_map(|path| {
            let result = if config.build.assets.contains_source(path) {
                process_asset(path, config, false, true)
            } else if path.starts_with(&config.build.content) {
                process_rel_asset(path, config, false, true)
            } else {
                process_asset(path, config, false, true)
            };
            result.err().map(|e| (path.clone(), e.to_string()))
        })
        .collect()
}

pub(super) fn cleanup_removed_assets(paths: &[PathBuf], config: &SiteConfig) -> usize {
    paths
        .iter()
        .filter(|path| !path.exists())
        .filter(|path| {
            let version_removed = crate::asset::version::remove_version(path);
            let output_removed = output_path_for_asset(path, config).is_some_and(|output| {
                let removed = remove_output_file(&output);
                if removed {
                    crate::debug!("assets"; "removed output for {}", path.display());
                }
                removed
            });

            if version_removed {
                crate::debug!("assets"; "removed version for {}", path.display());
            }

            version_removed || output_removed
        })
        .count()
}

fn output_path_for_asset(path: &Path, config: &SiteConfig) -> Option<PathBuf> {
    let output = config.paths().output_dir();

    if let Some(entry) = config
        .build
        .assets
        .flatten
        .iter()
        .find(|entry| path == entry.source())
    {
        return Some(output.join(entry.output_name()));
    }

    config.build.assets.nested.iter().find_map(|entry| {
        path.strip_prefix(entry.source())
            .ok()
            .map(|relative| output.join(entry.output_name()).join(relative))
    })
}

fn remove_output_file(output: &Path) -> bool {
    if !output.exists() {
        return false;
    }

    if let Err(e) = std::fs::remove_file(output) {
        crate::debug!("assets"; "failed to remove {}: {}", output.display(), e);
        return false;
    }

    remove_empty_parent(output);
    true
}

fn remove_empty_parent(output: &Path) {
    let Some(parent) = output.parent() else {
        return;
    };
    if parent
        .read_dir()
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::version::{self, ASSET_VERSIONS};
    use tempfile::TempDir;

    #[test]
    fn cleanup_removed_assets_deletes_output_and_version() {
        let dir = TempDir::new().unwrap();
        let root = crate::utils::path::normalize_path(dir.path());

        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.build.output = root.join("public");
        config.build.assets.normalize(&root);

        let source = root.join("assets").join("app.css");
        let output = root.join("public").join("assets").join("app.css");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&source, "body{}").unwrap();
        std::fs::write(&output, "body{}").unwrap();

        let _ = version::versioned_url("/assets/app.css", &source);
        assert!(ASSET_VERSIONS.contains_key(&crate::utils::path::normalize_path(&source)));

        std::fs::remove_file(&source).unwrap();

        let removed = cleanup_removed_assets(std::slice::from_ref(&source), &config);

        assert_eq!(removed, 1);
        assert!(!output.exists());
        assert!(!ASSET_VERSIONS.contains_key(&crate::utils::path::normalize_path(&source)));
        ASSET_VERSIONS.clear();
    }

    #[test]
    fn reloadable_output_asset_excludes_html() {
        assert!(is_reloadable_output_asset(Path::new(
            "/public/assets/app.css"
        )));
        assert!(is_reloadable_output_asset(Path::new(
            "/public/assets/app.js"
        )));
        assert!(!is_reloadable_output_asset(Path::new(
            "/public/page/index.html"
        )));
        assert!(!is_reloadable_output_asset(Path::new(
            "/public/page/index.htm"
        )));
    }
}

pub(super) fn log_asset_errors(errors: &[(PathBuf, String)]) {
    for (path, error) in errors {
        crate::log!("error"; "asset {}: {}", path.display(), error);
    }
}

pub(super) fn is_reloadable_output_asset(path: &Path) -> bool {
    !matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm")
    )
}

pub(super) fn format_asset_reason(total: usize, error_count: usize) -> String {
    if error_count == 0 {
        format!("{} assets updated", total)
    } else {
        format!(
            "{} assets updated, {} errors",
            total - error_count,
            error_count
        )
    }
}
