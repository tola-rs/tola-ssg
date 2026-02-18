//! UnoCSS specific hook builder.
//!
//! UnoCSS CLI arguments: `command [scan_patterns...] -o output [--minify]`

use crate::config::section::build::{CssProcessorConfig, HookConfig, WatchMode};
use anyhow::Result;
use std::path::Path;

/// Build a HookConfig for UnoCSS.
pub fn build_hook(css: &CssProcessorConfig, output: &Path, minify: bool) -> Result<HookConfig> {
    let mut command: Vec<String> = css.command.clone();

    // Add scan patterns if configured
    for pattern in &css.scan {
        command.push(pattern.clone());
    }

    // Add output: -o output
    command.extend(["-o".into(), output.display().to_string()]);

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
