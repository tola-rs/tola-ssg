//! Hook execution utilities.
//!
//! Provides environment variable building and command execution for build hooks.

use crate::config::SiteConfig;
use crate::config::section::build::HookConfig;
use crate::core::BuildMode;
use anyhow::Result;
use rustc_hash::FxHashMap;

// ============================================================================
// Environment Variables
// ============================================================================

/// Build `$TOLA_*` environment variables for hook execution
pub fn build_tola_vars(config: &SiteConfig, mode: BuildMode) -> FxHashMap<String, String> {
    let mut vars = FxHashMap::default();

    // Directory variables
    vars.insert(
        "TOLA_OUTPUT_DIR".into(),
        config.paths().output_dir().display().to_string(),
    );
    vars.insert("TOLA_ROOT".into(), config.get_root().display().to_string());

    // Mode variables
    let mode_str = if mode == BuildMode::PRODUCTION {
        "build"
    } else {
        "serve"
    };
    vars.insert("TOLA_MODE".into(), mode_str.into());
    vars.insert("TOLA_MINIFY".into(), config.build.minify.to_string());

    vars
}

// ============================================================================
// Command Argument Resolution
// ============================================================================

/// Resolve `$TOLA_*` variables in command arguments
///
/// Replaces occurrences of `$TOLA_XXX` with actual values from the vars map
pub fn resolve_args(args: &[String], vars: &FxHashMap<String, String>) -> Vec<String> {
    args.iter()
        .map(|arg| {
            let mut result = arg.clone();
            for (key, value) in vars {
                let pattern = format!("${}", key);
                result = result.replace(&pattern, value);
            }
            result
        })
        .collect()
}

// ============================================================================
// Hook Execution
// ============================================================================

/// Execute a single hook
///
/// The `phase` parameter is used for logging (e.g., "pre" or "post")
pub fn run_hook(
    hook: &HookConfig,
    config: &SiteConfig,
    mode: BuildMode,
    with_build_args: bool,
    phase: &str,
) -> Result<()> {
    use crate::utils::exec::{Cmd, SILENT_FILTER};

    if !hook.enable || hook.command.is_empty() {
        return Ok(());
    }

    let vars = build_tola_vars(config, mode);
    let mut resolved = resolve_args(&hook.command, &vars);

    // Append build_args if requested (typically during `tola build`)
    if with_build_args && !hook.build_args.is_empty() {
        let build_args = resolve_args(&hook.build_args, &vars);
        resolved.extend(build_args);
    }

    if !hook.quiet {
        crate::log!(phase; "`{}` running", hook.display_name());
    }

    let output = Cmd::from_slice(&resolved)
        .cwd(config.get_root())
        .envs(&vars)
        .pty(true)
        .filter(&SILENT_FILTER)
        .run()?;

    // Print output directly without prefix (unless quiet)
    if !hook.quiet {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stdout = stdout.trim();
        if !stdout.is_empty() {
            println!("{stdout}");
        }
    }

    Ok(())
}

/// Execute all pre hooks (including CSS processor if enabled)
pub fn run_pre_hooks(config: &SiteConfig, mode: BuildMode, with_build_args: bool) -> Result<()> {
    // User-defined pre hooks
    for hook in &config.build.hooks.pre {
        run_hook(hook, config, mode, with_build_args, "pre")?;
    }

    // CSS processor (syntax sugar for pre hook)
    if config.build.hooks.css.enable {
        super::css::rebuild_css(config, mode, with_build_args)?;
    }

    Ok(())
}

/// Execute all post hooks
pub fn run_post_hooks(config: &SiteConfig, mode: BuildMode, with_build_args: bool) -> Result<()> {
    for hook in &config.build.hooks.post {
        run_hook(hook, config, mode, with_build_args, "post")?;
    }
    Ok(())
}

// ============================================================================
// Watch Mode (serve)
// ============================================================================

use std::path::Path;

/// Check and execute hooks that match changed files (for serve mode)
///
/// Returns the number of hooks executed
pub fn run_watched_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> usize {
    let mut executed = 0;

    executed += run_watched_pre_hooks(config, changed_paths);
    executed += run_watched_post_hooks(config, changed_paths);

    executed
}

/// Execute pre hooks that match changed files
fn run_watched_pre_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> usize {
    let root = config.get_root();
    let mut executed = 0;

    // User-defined pre hooks
    for hook in &config.build.hooks.pre {
        if should_run_hook_for_changes(hook, changed_paths, root) {
            if let Err(e) = run_hook(hook, config, BuildMode::DEVELOPMENT, false, "pre") {
                crate::log!("hook"; "failed: {}", e);
            }
            executed += 1;
        }
    }

    // CSS processor (syntax sugar for pre hook)
    if config.build.hooks.css.enable
        && let Ok(css_hook) = super::css::build_css_hook(config, &css_output_path(config))
        && should_run_hook_for_changes(&css_hook, changed_paths, root)
    {
        if let Err(e) = run_hook(&css_hook, config, BuildMode::DEVELOPMENT, false, "pre") {
            crate::log!("css"; "failed: {}", e);
        }
        executed += 1;
    }

    executed
}

/// Get CSS output path for watch mode
fn css_output_path(config: &SiteConfig) -> std::path::PathBuf {
    config
        .build
        .hooks
        .css
        .path
        .as_ref()
        .and_then(|p| crate::asset::route_from_source(p.clone(), config).ok())
        .map(|r| r.output)
        .unwrap_or_default()
}

/// Execute post hooks that match changed files
fn run_watched_post_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> usize {
    let root = config.get_root();
    let mut executed = 0;

    for hook in &config.build.hooks.post {
        if should_run_hook_for_changes(hook, changed_paths, root) {
            if let Err(e) = run_hook(hook, config, BuildMode::DEVELOPMENT, false, "post") {
                crate::log!("hook"; "failed: {}", e);
            }
            executed += 1;
        }
    }

    executed
}

/// Check if a hook should run based on changed files
fn should_run_hook_for_changes(hook: &HookConfig, changed_paths: &[&Path], root: &Path) -> bool {
    if !hook.watch.is_enabled() {
        return false;
    }

    changed_paths
        .iter()
        .any(|path| hook.watch.matches(path, root))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_args_simple() {
        let mut vars = FxHashMap::default();
        vars.insert("TOLA_OUTPUT_DIR".into(), "/path/to/output".into());
        vars.insert("TOLA_ROOT".into(), "/path/to/root".into());

        let args = vec![
            "imagemin".into(),
            "$TOLA_OUTPUT_DIR/images".into(),
            "--out-dir".into(),
            "$TOLA_OUTPUT_DIR/images".into(),
        ];

        let resolved = resolve_args(&args, &vars);
        assert_eq!(resolved[0], "imagemin");
        assert_eq!(resolved[1], "/path/to/output/images");
        assert_eq!(resolved[3], "/path/to/output/images");
    }

    #[test]
    fn test_resolve_args_no_vars() {
        let vars = FxHashMap::default();
        let args = vec!["echo".into(), "hello".into()];
        let resolved = resolve_args(&args, &vars);
        assert_eq!(resolved, args);
    }

    #[test]
    fn test_resolve_args_multiple_vars_in_one_arg() {
        let mut vars = FxHashMap::default();
        vars.insert("TOLA_ROOT".into(), "/root".into());
        vars.insert("TOLA_OUTPUT_DIR".into(), "/output".into());

        let args = vec!["cp $TOLA_ROOT/src $TOLA_OUTPUT_DIR/dest".into()];
        let resolved = resolve_args(&args, &vars);
        assert_eq!(resolved[0], "cp /root/src /output/dest");
    }
}
