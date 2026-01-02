//! CSS utilities: CSS processor integration.
//!
//! This module provides CSS processor (e.g., Tailwind) build integration.
//! CSS processor is syntax sugar for a pre hook.

use crate::config::SiteConfig;
use crate::config::section::build::{HookConfig, WatchMode};
use crate::core::BuildMode;
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

// ============================================================================
// CSS Processor
// ============================================================================

/// Check if a path is the CSS processor input file.
pub fn is_css_processor_input(path: &Path, config: &SiteConfig) -> bool {
    config.build.css.processor.enable
        && config
            .build
            .css
            .processor
            .input
            .as_ref()
            .is_some_and(|input| {
                // Compare normalized paths (input is already normalized in config)
                crate::utils::path::normalize_path(path) == *input
            })
}

/// Build a HookConfig from CSS processor configuration.
pub fn build_css_processor_hook(config: &SiteConfig, output: &Path) -> Result<HookConfig> {
    let processor = &config.build.css.processor;
    let input = processor
        .input
        .as_ref()
        .ok_or_else(|| anyhow!("CSS processor input not configured"))?;

    // Build command: command + ["-i", input, "-o", output]
    let mut command: Vec<String> = processor.command.clone();
    command.extend([
        "-i".into(),
        input.display().to_string(),
        "-o".into(),
        output.display().to_string(),
    ]);

    // Build args: --minify (only in build mode)
    let build_args = if config.build.minify {
        vec!["--minify".into()]
    } else {
        vec![]
    };

    Ok(HookConfig {
        enable: true,
        name: Some("css".into()),
        command,
        watch: WatchMode::Bool(true),
        build_args,
        quiet: processor.quiet,
    })
}

/// Run CSS processor as a pre hook.
pub fn run_css_processor(
    config: &SiteConfig,
    output: &Path,
    mode: BuildMode,
    with_build_args: bool,
) -> Result<()> {
    // Ensure output directory exists (CSS processor runs before assets are copied)
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let hook = build_css_processor_hook(config, output)?;
    super::hooks::run_hook(&hook, config, mode, with_build_args, "pre")
}

/// Rebuild CSS using configured input path.
///
/// Used by watch mode to rebuild when source files change.
pub fn rebuild_css_processor(
    config: &SiteConfig,
    get_output_path: impl FnOnce(&Path) -> Result<PathBuf>,
    mode: BuildMode,
    with_build_args: bool,
) -> Result<()> {
    let input = config
        .build
        .css
        .processor
        .input
        .as_ref()
        .ok_or_else(|| anyhow!("CSS processor input path not configured"))?;

    let output = get_output_path(input)?;
    run_css_processor(config, &output, mode, with_build_args)
}
