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
