//! Build hooks configuration.
//!
//! # Example
//!
//! ```toml
//! # Pre hooks (run before build)
//! [[build.hooks.pre]]
//! command = ["./scripts/gen-icons.sh"]
//! watch = ["assets/icons/**"]
//!
//! [[build.hooks.pre]]
//! command = ["esbuild", "src/app.ts", "--bundle", "--outfile=$TOLA_OUTPUT_DIR/assets/js/app.js"]
//! build_args = ["--minify"]
//!
//! # Post hooks (run after build)
//! [[build.hooks.post]]
//! command = ["imagemin", "$TOLA_OUTPUT_DIR/images", "--out-dir", "$TOLA_OUTPUT_DIR/images"]
//! ```

use serde::{Deserialize, Serialize};

/// Hooks configuration containing pre and post build hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Pre-build hooks (run before content compilation).
    pub pre: Vec<HookConfig>,
    /// Post-build hooks (run after build completion).
    pub post: Vec<HookConfig>,
}

/// Configuration for a single build hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookConfig {
    /// Whether this hook is enabled (default: true).
    #[serde(default = "default_enable")]
    pub enable: bool,

    /// Display name for logging (defaults to command[0]).
    pub name: Option<String>,

    /// Command and arguments to execute.
    /// Supports `$TOLA_*` variable substitution.
    pub command: Vec<String>,

    /// Watch mode for serve (re-execute on file changes).
    #[serde(default)]
    pub watch: WatchMode,

    /// Additional arguments appended only during `tola build` (not serve).
    #[serde(default)]
    pub build_args: Vec<String>,

    /// Suppress output (default: true).
    #[serde(default = "default_quiet")]
    pub quiet: bool,
}

fn default_quiet() -> bool {
    true
}

fn default_enable() -> bool {
    true
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            enable: true,
            name: None,
            command: Vec::new(),
            watch: WatchMode::default(),
            build_args: Vec::new(),
            quiet: true,
        }
    }
}

impl HookConfig {
    /// Get the display name for this hook.
    ///
    /// Returns `name` if set, otherwise falls back to `command[0]`.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .unwrap_or_else(|| self.command.first().map(String::as_str).unwrap_or("hook"))
    }
}

/// Watch mode for hooks in serve mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WatchMode {
    /// Disabled (default).
    #[default]
    #[serde(skip)]
    Disabled,
    /// Boolean: true = always re-execute, false = disabled.
    Bool(bool),
    /// Glob patterns: re-execute when matching files change.
    Patterns(Vec<String>),
}

impl WatchMode {
    /// Check if watch is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            WatchMode::Disabled => false,
            WatchMode::Bool(b) => *b,
            WatchMode::Patterns(p) => !p.is_empty(),
        }
    }

    /// Check if a path matches this watch mode.
    ///
    /// - `Disabled` / `Bool(false)`: never matches
    /// - `Bool(true)`: always matches (any file change triggers)
    /// - `Patterns(paths)`: matches if path ends with any of the patterns
    pub fn matches(&self, path: &std::path::Path, root: &std::path::Path) -> bool {
        match self {
            WatchMode::Disabled => false,
            WatchMode::Bool(b) => *b,
            WatchMode::Patterns(patterns) => {
                // Get relative path from root
                let rel_path = path.strip_prefix(root).unwrap_or(path);
                let rel_str = rel_path.to_string_lossy();

                patterns
                    .iter()
                    .any(|pattern| rel_str == *pattern || rel_str.ends_with(pattern))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_parse_config;

    #[test]
    fn test_empty_hooks() {
        let config = test_parse_config("");
        assert!(config.build.hooks.pre.is_empty());
        assert!(config.build.hooks.post.is_empty());
    }

    #[test]
    fn test_pre_hook() {
        let config = test_parse_config(
            r#"
[[build.hooks.pre]]
command = ["echo", "hello"]
"#,
        );
        assert_eq!(config.build.hooks.pre.len(), 1);
        let hook = &config.build.hooks.pre[0];
        assert_eq!(hook.command, vec!["echo", "hello"]);
        assert_eq!(hook.display_name(), "echo");
    }

    #[test]
    fn test_post_hook_with_watch() {
        let config = test_parse_config(
            r#"
[[build.hooks.post]]
command = ["imagemin", "$TOLA_OUTPUT_DIR"]
watch = true
"#,
        );
        assert_eq!(config.build.hooks.post.len(), 1);
        let hook = &config.build.hooks.post[0];
        assert_eq!(hook.command, vec!["imagemin", "$TOLA_OUTPUT_DIR"]);
        assert!(hook.watch.is_enabled());
    }

    #[test]
    fn test_watch_patterns() {
        let config = test_parse_config(
            r#"
[[build.hooks.pre]]
command = ["gen-icons"]
watch = ["assets/icons/**"]
"#,
        );
        let hook = &config.build.hooks.pre[0];
        match &hook.watch {
            WatchMode::Patterns(p) => assert_eq!(p, &vec!["assets/icons/**"]),
            _ => panic!("Expected patterns"),
        }
    }

    #[test]
    fn test_build_args() {
        let config = test_parse_config(
            r#"
[[build.hooks.pre]]
command = ["esbuild", "src/app.ts"]
build_args = ["--minify", "--sourcemap"]
"#,
        );
        let hook = &config.build.hooks.pre[0];
        assert_eq!(hook.build_args, vec!["--minify", "--sourcemap"]);
    }
}
