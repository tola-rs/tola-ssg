//! Hook execution utilities.
//!
//! Provides environment variable building and command execution for build hooks.

use crate::config::SiteConfig;
use crate::config::section::build::HookConfig;
use crate::core::BuildMode;
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::path::PathBuf;

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
    for entry in collect_pre_hooks(config)? {
        run_hook(&entry.hook, config, mode, with_build_args, "pre")?;
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
/// Returns execution summary for no-op-safe recompilation decisions.
#[derive(Debug, Clone, Default)]
pub struct WatchedHooksResult {
    /// Total hooks executed (pre + post + sugar hooks).
    pub executed: usize,
    /// Number of hooks whose side effects are not precisely tracked.
    pub conservative_executed: usize,
    /// Output files for hooks with explicitly tracked side effects.
    pub tracked_outputs: Vec<PathBuf>,
}

/// Check and execute hooks that match changed files (for serve mode).
pub fn run_watched_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> WatchedHooksResult {
    let mut result = WatchedHooksResult::default();

    run_watched_pre_hooks(config, changed_paths, &mut result);
    run_watched_post_hooks(config, changed_paths, &mut result);

    result
}

/// Execute pre hooks that match changed files
fn run_watched_pre_hooks(
    config: &SiteConfig,
    changed_paths: &[&Path],
    result: &mut WatchedHooksResult,
) {
    let root = config.get_root();
    let pre_hooks = match collect_pre_hooks(config) {
        Ok(hooks) => hooks,
        Err(e) => {
            crate::log!("hook"; "failed to build pre hooks: {}", e);
            config
                .build
                .hooks
                .pre
                .iter()
                .cloned()
                .map(|hook| PreHookEntry {
                    hook,
                    tracked_output: None,
                })
                .collect()
        }
    };

    for entry in pre_hooks {
        if should_run_hook_for_changes(&entry.hook, changed_paths, root) {
            if let Err(e) = run_hook(&entry.hook, config, BuildMode::DEVELOPMENT, false, "pre") {
                crate::log!("hook"; "failed: {}", e);
            }
            result.executed += 1;
            if let Some(output) = entry.tracked_output {
                result.tracked_outputs.push(output);
            } else {
                result.conservative_executed += 1;
            }
        }
    }
}

/// Execute post hooks that match changed files
fn run_watched_post_hooks(
    config: &SiteConfig,
    changed_paths: &[&Path],
    result: &mut WatchedHooksResult,
) {
    let root = config.get_root();

    for hook in &config.build.hooks.post {
        if should_run_hook_for_changes(hook, changed_paths, root) {
            if let Err(e) = run_hook(hook, config, BuildMode::DEVELOPMENT, false, "post") {
                crate::log!("hook"; "failed: {}", e);
            }
            result.executed += 1;
            result.conservative_executed += 1;
        }
    }
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

/// Collect user pre hooks plus optional CSS syntax-sugar hook.
fn collect_pre_hooks(config: &SiteConfig) -> Result<Vec<PreHookEntry>> {
    let mut hooks: Vec<PreHookEntry> = config
        .build
        .hooks
        .pre
        .iter()
        .cloned()
        .map(|hook| PreHookEntry {
            hook,
            tracked_output: None,
        })
        .collect();

    if let Some(output) = super::css::css_output_path(config)? {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let css_hook = super::css::build_css_hook(config, &output)?;
        hooks.push(PreHookEntry {
            hook: css_hook,
            tracked_output: Some(output),
        });
    }

    Ok(hooks)
}

#[derive(Debug, Clone)]
struct PreHookEntry {
    hook: HookConfig,
    tracked_output: Option<PathBuf>,
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
