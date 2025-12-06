use crate::config::SiteConfig;
use crate::compiler::meta::AssetMeta;
use anyhow::Result;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

static ASSET_TOP_LEVELS: OnceLock<HashSet<OsString>> = OnceLock::new();

/// Get MIME type for icon based on file extension
pub fn get_icon_mime_type(path: &Path) -> &'static str {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "ico" => "image/x-icon",
            "png" => "image/png",
            "svg" => "image/svg+xml",
            "avif" => "image/avif",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "jpg" | "jpeg" => "image/jpeg",
            _ => "image/x-icon",
        })
        .unwrap_or("image/x-icon")
}

/// Compute href for an asset path relative to path_prefix
pub fn compute_asset_href(asset_path: &Path, config: &SiteConfig) -> Result<String> {
    let assets_dir = &config.build.assets;
    // Strip the leading "./" prefix if present
    let without_dot_prefix = asset_path.strip_prefix("./").unwrap_or(asset_path);
    // Strip the "assets/" prefix if present to get relative path within assets
    let relative_path = without_dot_prefix
        .strip_prefix("assets/")
        .unwrap_or(without_dot_prefix);

    let source = assets_dir.join(relative_path);
    let meta = AssetMeta::from_source(source, config)?;
    Ok(meta.paths.url)
}

/// Compute stylesheet href from input path
pub fn compute_stylesheet_href(input: &Path, config: &SiteConfig) -> Result<String> {
    let input = input.canonicalize()?;
    let meta = AssetMeta::from_source(input, config)?;
    Ok(meta.paths.url)
}

/// Get top-level asset directory names
fn get_asset_top_levels(assets_dir: &Path) -> &'static HashSet<OsString> {
    ASSET_TOP_LEVELS.get_or_init(|| {
        fs::read_dir(assets_dir)
            .map(|dir| dir.flatten().map(|entry| entry.file_name()).collect())
            .unwrap_or_default()
    })
}

/// Check if a path is an asset link
pub fn is_asset_link(path: &str, config: &'static SiteConfig) -> bool {
    let asset_top_levels = get_asset_top_levels(&config.build.assets);

    // Extract first path component after the leading slash
    let first_component = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();

    asset_top_levels.contains(first_component.as_ref() as &std::ffi::OsStr)
}
