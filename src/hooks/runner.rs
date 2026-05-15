//! Hook execution utilities.
//!
//! Provides command execution for build hooks.

use crate::config::SiteConfig;
use crate::config::section::build::HookConfig;
use anyhow::Result;

// ============================================================================
// Hook Execution
// ============================================================================

/// Build hook execution phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPhase {
    Pre,
    Post,
}

impl HookPhase {
    /// String form used by logging targets.
    pub const fn as_str(self) -> &'static str {
        match self {
            HookPhase::Pre => "pre",
            HookPhase::Post => "post",
        }
    }
}

/// Execute a single hook
///
/// The `phase` parameter is used for logging labels.
pub fn run_hook(hook: &HookConfig, config: &SiteConfig, phase: HookPhase) -> Result<()> {
    use crate::utils::exec::{Cmd, SILENT_FILTER};

    if !hook.enable || hook.command.is_empty() {
        return Ok(());
    }

    if !hook.quiet {
        crate::log!(phase.as_str(); "`{}` running", hook.display_name());
    }

    let output = Cmd::from_slice(&hook.command)
        .cwd(config.get_root())
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

/// Execute all pre hooks.
pub fn run_pre_hooks(config: &SiteConfig) -> Result<()> {
    for hook in &config.build.hooks.pre {
        run_hook(hook, config, HookPhase::Pre)?;
    }

    Ok(())
}

/// Execute all post hooks
pub fn run_post_hooks(config: &SiteConfig) -> Result<()> {
    for hook in &config.build.hooks.post {
        run_hook(hook, config, HookPhase::Post)?;
    }
    Ok(())
}

// ============================================================================
// Watch Mode (serve)
// ============================================================================

use std::path::Path;

/// Check and execute hooks that match changed files (for serve mode)
///
/// Returns the number of hooks executed.
pub fn run_watched_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> usize {
    run_watched_pre_hooks(config, changed_paths) + run_watched_post_hooks(config, changed_paths)
}

/// Check if any watched hook would run for changed files.
///
/// This is a pure predicate used by file-event routing to decide whether
/// to enqueue a hook-only compile cycle.
pub fn has_watched_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> bool {
    has_watched_pre_hooks(config, changed_paths) || has_watched_post_hooks(config, changed_paths)
}

/// Check whether any watched pre hook would run for changed files.
pub fn has_watched_pre_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> bool {
    let root = config.get_root();
    has_matching_hook(config.build.hooks.pre.iter(), changed_paths, root)
}

/// Check whether any watched post hook would run for changed files.
pub fn has_watched_post_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> bool {
    let root = config.get_root();
    has_matching_hook(config.build.hooks.post.iter(), changed_paths, root)
}

/// Execute pre hooks that match changed files
pub fn run_watched_pre_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> usize {
    let root = config.get_root();
    run_watched_hook_set(
        config.build.hooks.pre.iter(),
        config,
        changed_paths,
        root,
        HookPhase::Pre,
    )
}

/// Execute post hooks that match changed files
pub fn run_watched_post_hooks(config: &SiteConfig, changed_paths: &[&Path]) -> usize {
    let root = config.get_root();
    run_watched_hook_set(
        config.build.hooks.post.iter(),
        config,
        changed_paths,
        root,
        HookPhase::Post,
    )
}

fn run_watched_hook_set<'a>(
    hooks: impl Iterator<Item = &'a HookConfig>,
    config: &SiteConfig,
    changed_paths: &[&Path],
    root: &Path,
    phase: HookPhase,
) -> usize {
    let mut executed = 0;

    for hook in hooks {
        if should_run_hook_for_changes(hook, changed_paths, root) {
            if let Err(e) = run_hook(hook, config, phase) {
                crate::log!("hook"; "failed: {}", e);
            }
            executed += 1;
        }
    }

    executed
}

fn has_matching_hook<'a>(
    mut hooks: impl Iterator<Item = &'a HookConfig>,
    changed_paths: &[&Path],
    root: &Path,
) -> bool {
    hooks.any(|hook| should_run_hook_for_changes(hook, changed_paths, root))
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
    use crate::config::section::build::WatchMode;

    #[test]
    fn test_has_watched_hooks_split_by_phase() {
        let mut config = SiteConfig::default();
        config.set_root(std::path::Path::new("/site"));
        config.build.hooks.pre.push(HookConfig {
            command: vec!["echo".into()],
            watch: WatchMode::Patterns(vec!["assets/pre.css".into()]),
            ..HookConfig::default()
        });
        config.build.hooks.post.push(HookConfig {
            command: vec!["echo".into()],
            watch: WatchMode::Patterns(vec!["assets/post.css".into()]),
            ..HookConfig::default()
        });

        let pre = std::path::Path::new("/site/assets/pre.css");
        let post = std::path::Path::new("/site/assets/post.css");
        let pre_refs = vec![pre];
        let post_refs = vec![post];

        assert!(has_watched_pre_hooks(&config, &pre_refs));
        assert!(!has_watched_post_hooks(&config, &pre_refs));
        assert!(has_watched_post_hooks(&config, &post_refs));
        assert!(!has_watched_pre_hooks(&config, &post_refs));
        assert!(has_watched_hooks(&config, &pre_refs));
        assert!(has_watched_hooks(&config, &post_refs));
    }
}
