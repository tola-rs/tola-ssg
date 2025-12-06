use crate::config::SiteConfig;
use crate::utils::exec::FilterRule;
use crate::compiler::meta::AssetMeta;
use crate::compiler::is_up_to_date;
use crate::{exec, log};
use anyhow::{Result, anyhow};
use std::fs;
use std::path::Path;

/// Tailwind filter: skip version banner.
const TAILWIND_FILTER: FilterRule = FilterRule::new(&["≈ tailwindcss"]);

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
    if ext == "css"
        && config.build.tailwind.enable
        && let Some(input) = &config.build.tailwind.input
        && asset_path.canonicalize().ok().as_ref() == Some(input)
    {
        return run_tailwind(input, &meta.paths.dest, config);
    }

    // Default: copy file
    fs::copy(&meta.paths.source, &meta.paths.dest)?;
    Ok(())
}

/// Process an asset file from the content directory (non-.typ files).
pub fn process_relative_asset(
    path: &Path,
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<()> {
    let content = &config.build.content;
    let output = &config.build.output.join(&config.build.path_prefix);

    let relative_path = path
        .strip_prefix(content)?
        .to_str()
        .ok_or(anyhow!("Invalid path"))?;

    let output_path = output.join(relative_path);

    // Relative assets don't depend on templates/config, just check source vs dest
    if !clean && is_up_to_date(path, &output_path, None) {
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

    run_tailwind(input, &meta.paths.dest, config)?;

    Ok(())
}

fn run_tailwind(input: &Path, output: &Path, config: &SiteConfig) -> Result<()> {
    exec!(
        pty=true;
        filter=&TAILWIND_FILTER;
        config.get_root();
        &config.build.tailwind.command;
        "-i", input, "-o", output,
        if config.build.minify { "--minify" } else { "" }
    )?;
    Ok(())
}
