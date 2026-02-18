//! Tailwind CSS specific hook builder.
//!
//! Tailwind CLI arguments: `command -i input -o output [--minify]`

use crate::config::section::build::{CssProcessorConfig, HookConfig, WatchMode};
use anyhow::{anyhow, Result};
use std::path::Path;

/// Build a HookConfig for Tailwind CSS
pub fn build_hook(css: &CssProcessorConfig, output: &Path, minify: bool) -> Result<HookConfig> {
    let input = css
        .path
        .as_ref()
        .ok_or_else(|| anyhow!("CSS path not configured"))?;

    // Build command: command + ["-i", input, "-o", output]
    let mut command: Vec<String> = css.command.clone();
    command.extend([
        "-i".into(),
        input.display().to_string(),
        "-o".into(),
        output.display().to_string(),
    ]);

    // Build args: --minify (only in build mode)
    let build_args = if minify {
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
        quiet: css.quiet,
    })
}
