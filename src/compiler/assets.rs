use crate::config::SiteConfig;
use crate::utils::exec::FilterRule;
use crate::utils::meta::AssetMeta;
use crate::compiler::utils::is_up_to_date;
use crate::{exec, log};
use anyhow::{Result, anyhow};
use std::fs;
use std::path::Path;

/// Tailwind filter: skip version banner.
const TAILWIND_FILTER: FilterRule = FilterRule::new(&["≈ tailwindcss"]);

pub fn process_asset(
    asset_path: &Path,
    config: &'static SiteConfig,
    log_file: bool,
) -> Result<()> {
    let meta = AssetMeta::from_source(asset_path.to_path_buf(), config)?;

    if log_file {
        log!("assets"; "{}", meta.relative);
    }

    if let Some(parent) = meta.dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let asset_extension = asset_path
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();

    match asset_extension {
        "css" if config.build.tailwind.enable => {
            let input = config.build.tailwind.input.as_ref().unwrap();
            // Config paths are already absolute, just canonicalize the runtime path
            let asset_path = asset_path.canonicalize().unwrap();
            if *input == asset_path {
                run_tailwind(input, &meta.dest, config)?;
            } else {
                fs::copy(&meta.source, &meta.dest)?;
            }
        }
        _ => {
            fs::copy(&meta.source, &meta.dest)?;
        }
    }

    Ok(())
}

pub fn process_relative_asset(
    path: &Path,
    config: &SiteConfig,
    force_rebuild: bool,
    log_file: bool,
) -> Result<()> {
    // Relative assets are in content directory, but we treat them similar to assets
    // However, AssetMeta expects them to be in assets directory.
    // So we can't use AssetMeta::from_source directly if it enforces assets dir check.
    // Let's check AssetMeta implementation.
    // It does: source.strip_prefix(assets_dir)

    // So for relative assets (in content dir), we need manual logic or a different helper.
    // Let's keep manual logic for now but use url_from_output_path if needed (not needed here, just copy).

    let content = &config.build.content;
    let output = &config.build.output.join(&config.build.path_prefix);

    let relative_path = path
        .strip_prefix(content)?
        .to_str()
        .ok_or(anyhow!("Invalid path"))?;

    let output_path = output.join(relative_path);

    // Relative assets don't depend on templates/config, just check source vs dest
    if !force_rebuild && is_up_to_date(path, &output_path, None) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", relative_path);
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(path, output_path)?;
    Ok(())
}

/// Rebuild tailwind CSS
pub fn rebuild_tailwind(config: &'static SiteConfig) -> Result<()> {
    let input = config
        .build
        .tailwind
        .input
        .as_ref()
        .ok_or_else(|| anyhow!("Tailwind input path not configured"))?;

    // We can use AssetMeta here if input is in assets dir
    let meta = AssetMeta::from_source(input.clone(), config)?;

    run_tailwind(input, &meta.dest, config)?;

    Ok(())
}

fn run_tailwind(input: &Path, output: &Path, config: &SiteConfig) -> Result<()> {
    exec!(
        filter=&TAILWIND_FILTER;
        config.get_root();
        &config.build.tailwind.command;
        "-i", input, "-o", output,
        if config.build.minify { "--minify" } else { "" }
    )?;
    Ok(())
}
