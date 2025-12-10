use crate::config::SiteConfig;
use crate::compiler::meta::AssetMeta;
use crate::compiler::is_up_to_date;
use crate::utils::css;
use crate::log;
use anyhow::{Result, anyhow};
use std::fs;
use std::path::Path;

/// Process an asset file from the assets directory.
pub fn process_asset(
    asset_path: &Path,
    config: &'static SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<()> {
    let meta = AssetMeta::from_source(asset_path.to_path_buf(), config)?;

    // Skip if up-to-date (assets don't depend on templates)
    if !clean && is_up_to_date(asset_path, &meta.paths.dest, None) {
        return Ok(());
    }

    if log_file {
        log!("assets"; "{}", meta.paths.relative);
    }

    if let Some(parent) = meta.paths.dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let ext = asset_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

    // Handle tailwind CSS specially
    if ext == "css" && css::is_tailwind_input(asset_path, config) {
        return css::run_tailwind(asset_path, &meta.paths.dest, config);
    }

    // Default: copy file
    fs::copy(&meta.paths.source, &meta.paths.dest)?;
    Ok(())
}

/// Process an asset file from the content directory (non-.typ files).
pub fn process_rel_asset(
    path: &Path,
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<()> {
    let content = &config.build.content;
    let output = &config.build.output.join(&config.build.path_prefix);

    let rel_path = path
        .strip_prefix(content)?
        .to_str()
        .ok_or_else(|| anyhow!("Invalid path"))?;

    let output_path = output.join(rel_path);

    // Relative assets don't depend on templates/config, just check source vs dest
    if !clean && is_up_to_date(path, &output_path, None) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", rel_path);
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(path, output_path)?;
    Ok(())
}

/// Rebuild tailwind CSS.
///
/// Delegates to `utils::css::rebuild_tailwind` with asset path resolution.
pub fn rebuild_tailwind(config: &'static SiteConfig) -> Result<()> {
    css::rebuild_tailwind(config, |input| {
        let meta = AssetMeta::from_source(input.to_path_buf(), config)?;
        Ok(meta.paths.dest)
    })
}
