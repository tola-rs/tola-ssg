//! CSS processor integrations.
//!
//! Supported processors:
//! - `tailwind`: Tailwind CSS
//! - `uno`: UnoCSS

mod tailwind;
mod uno;

use crate::config::SiteConfig;
use crate::config::section::build::{CssFormat, HookConfig};
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

/// Check if a path is the CSS processor path file
pub fn is_css_input(path: &Path, config: &SiteConfig) -> bool {
    config.build.hooks.css.enable
        && config
            .build
            .hooks
            .css
            .path
            .as_ref()
            .is_some_and(|p| crate::utils::path::normalize_path(path) == *p)
}

/// Build a HookConfig from CSS processor configuration
pub fn build_css_hook(config: &SiteConfig, output: &Path) -> Result<HookConfig> {
    let css = &config.build.hooks.css;
    let minify = config.build.minify;

    match css.resolved_format() {
        CssFormat::Tailwind => tailwind::build_hook(css, output, minify),
        CssFormat::Uno => uno::build_hook(css, output, minify),
        CssFormat::Auto => unreachable!("resolved_format() should never return Auto"),
    }
}

/// Resolve CSS output path from `build.hooks.css.path`.
///
/// Returns `Ok(None)` when CSS processor is disabled.
pub fn css_output_path(config: &SiteConfig) -> Result<Option<PathBuf>> {
    if !config.build.hooks.css.enable {
        return Ok(None);
    }

    let path = config
        .build
        .hooks
        .css
        .path
        .as_ref()
        .ok_or_else(|| anyhow!("CSS processor path not configured"))?;

    let route = crate::asset::route_from_source(path.to_path_buf(), config)?;
    Ok(Some(route.output))
}
