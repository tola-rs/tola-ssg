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
//!
//! # CSS processor (syntax sugar for pre hook)
//! [build.hooks.css]
//! enable = true
//! input = "assets/css/main.css"
//! command = ["tailwindcss"]
//! ```

use crate::config::ConfigDiagnostics;
use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Hooks configuration containing pre and post build hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Pre-build hooks (run before content compilation).
    pub pre: Vec<HookConfig>,
    /// Post-build hooks (run after build completion).
    pub post: Vec<HookConfig>,
    /// CSS processor hook (syntax sugar for pre hook).
    pub css: CssProcessorConfig,
}

impl HooksConfig {
    /// Validate hooks configuration.
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        self.css.validate(diag);
    }
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

// ============================================================================
// CSS Processor Config
// ============================================================================

/// CSS processor format (determines CLI arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CssFormat {
    /// Auto-detect from command (default).
    #[default]
    Auto,
    /// Tailwind CSS: `-i input -o output [--minify]`
    Tailwind,
    /// UnoCSS: `input -o output [--minify]`
    Uno,
}

impl CssFormat {
    /// Infer format from command.
    pub fn infer_from_command(command: &[String]) -> Self {
        let cmd_str = command.join(" ").to_lowercase();
        if cmd_str.contains("unocss") || cmd_str.contains("uno") {
            CssFormat::Uno
        } else {
            CssFormat::Tailwind // default
        }
    }

    /// Resolve auto to concrete format.
    pub fn resolve(&self, command: &[String]) -> Self {
        match self {
            CssFormat::Auto => Self::infer_from_command(command),
            _ => *self,
        }
    }
}

/// CSS processor hook configuration (syntax sugar for pre hook).
///
/// When enabled, this is internally compiled to a pre hook with:
/// - Auto-expanded command with format-specific arguments
/// - `watch = true` (any file change triggers rebuild)
/// - `build_args` for minification
///
/// # Example
///
/// ```toml
/// # Tailwind CSS
/// [build.hooks.css]
/// enable = true
/// path = "assets/css/main.css"
/// command = ["tailwindcss"]
///
/// # UnoCSS with scan patterns
/// [build.hooks.css]
/// enable = true
/// path = "assets/css/uno.css"
/// command = ["unocss"]
/// scan = ["content/**/*", "templates/**/*"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.hooks.css")]
pub struct CssProcessorConfig {
    /// Enable CSS processor hook.
    pub enable: bool,
    /// Output asset path (also used as Tailwind input file location).
    pub path: Option<PathBuf>,
    /// Command to execute (e.g., `["tailwindcss"]` or `["unocss"]`).
    pub command: Vec<String>,
    /// CSS processor format (auto, tailwind, uno). Default: auto (inferred from command).
    pub format: CssFormat,
    /// Glob patterns for scanning source files (UnoCSS only).
    #[serde(default)]
    pub scan: Vec<String>,
    /// Suppress output (default: true).
    pub quiet: bool,
}

impl Default for CssProcessorConfig {
    fn default() -> Self {
        Self {
            enable: false,
            path: None,
            command: vec!["tailwindcss".into()],
            format: CssFormat::Auto,
            scan: Vec::new(),
            quiet: true,
        }
    }
}

impl CssProcessorConfig {
    /// Get the resolved format (auto → concrete).
    pub fn resolved_format(&self) -> CssFormat {
        self.format.resolve(&self.command)
    }

    /// Validate CSS processor configuration.
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        if !self.enable {
            return;
        }

        // Command must have at least one element
        if self.command.is_empty() {
            diag.error(
                Self::FIELDS.command,
                format!(
                    "{} is true but {} is empty",
                    Self::FIELDS.enable,
                    Self::FIELDS.command
                ),
            );
            return;
        }

        // Check if command is installed
        let cmd = &self.command[0];
        let is_package_runner = ["npx", "bunx", "pnpx", "yarn", "dlx"].contains(&cmd.as_str());

        if which::which(cmd).is_err() {
            if is_package_runner {
                // Package runners can download packages at runtime, just hint
                if self.command.len() > 1 {
                    diag.hint(
                        Self::FIELDS.command,
                        format!(
                            "`{}` via `{}` — ensure package is installed",
                            self.command[1], cmd
                        ),
                    );
                }
            } else {
                diag.error_with_hint(
                    Self::FIELDS.command,
                    format!("`{cmd}` not found"),
                    format!("install the command or update {}", Self::FIELDS.command),
                );
            }
        }

        // Path must be configured
        let Some(path) = &self.path else {
            diag.error(
                Self::FIELDS.path,
                format!(
                    "{} is true but {} is not configured",
                    Self::FIELDS.enable,
                    Self::FIELDS.path
                ),
            );
            return;
        };

        // For Tailwind, path must exist as input file
        // For UnoCSS, path is just output location (file doesn't need to exist)
        if self.resolved_format() == CssFormat::Tailwind {
            if !path.exists() {
                diag.error(
                    Self::FIELDS.path,
                    format!("{} file not found: {}", Self::FIELDS.path, path.display()),
                );
            } else if !path.is_file() {
                diag.error(
                    Self::FIELDS.path,
                    format!("{} is not a file: {}", Self::FIELDS.path, path.display()),
                );
            }
        }
    }
}
