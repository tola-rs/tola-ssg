//! CSS processor integrations.
//!
//! Supported processors:
//! - `tailwind`: Tailwind CSS
//! - `uno`: UnoCSS (planned)

mod tailwind;

use crate::config::SiteConfig;
use crate::config::section::build::{CssFormat, HookConfig};
use crate::core::BuildMode;
use anyhow::{anyhow, Result};
use std::path::Path;

/// Check if a path is the CSS processor input file.
pub fn is_css_input(path: &Path, config: &SiteConfig) -> bool {
    config.build.hooks.css.enable
        && config
            .build
            .hooks
            .css
            .input
            .as_ref()
            .is_some_and(|input| {
                crate::utils::path::normalize_path(path) == *input
            })
}

/// Build a HookConfig from CSS processor configuration.
pub fn build_css_hook(config: &SiteConfig, output: &Path) -> Result<HookConfig> {
    let css = &config.build.hooks.css;
    let minify = config.build.minify;

    match css.resolved_format() {
        CssFormat::Tailwind => tailwind::build_hook(css, output, minify),
        CssFormat::Uno => tailwind::build_hook(css, output, minify), // TODO: uno::build_hook
        CssFormat::Auto => unreachable!("resolved_format() should never return Auto"),
    }
}

/// Run CSS processor as a pre hook.
pub fn run_css(
    config: &SiteConfig,
    output: &Path,
    mode: BuildMode,
    with_build_args: bool,
) -> Result<()> {
    // Ensure output directory exists
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let hook = build_css_hook(config, output)?;
    crate::hooks::run_hook(&hook, config, mode, with_build_args, "pre")?;

    Ok(())
}

/// Rebuild CSS using configured input path.
pub fn rebuild_css(
    config: &SiteConfig,
    mode: BuildMode,
    with_build_args: bool,
) -> Result<()> {
    let input = config
        .build
        .hooks
        .css
        .input
        .as_ref()
        .ok_or_else(|| anyhow!("CSS processor input path not configured"))?;

    let route = crate::asset::route_from_source(input.to_path_buf(), config)?;
    run_css(config, &route.output, mode, with_build_args)
}
