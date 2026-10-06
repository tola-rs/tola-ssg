//! Build hooks configuration.
//!
//! # Example
//!
//! ```toml
//! # Pre hooks (run before build)
//! [[build.hooks.pre]]
//! command = ["./scripts/gen-icons.sh"]
//! watch = ["assets/icons"]
//!
//! [[build.hooks.pre]]
//! command = ["esbuild", "src/app.ts", "--bundle", "--outfile=public/assets/js/app.js"]
//!
//! # Post hooks (run after build)
//! [[build.hooks.post]]
//! command = ["imagemin", "public/images", "--out-dir", "public/images"]
//!
//! ```

use crate::config::ConfigDiagnostics;
use serde::{Deserialize, Serialize};

/// Hooks configuration containing pre and post build hooks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Pre-build hooks (run before content compilation).
    pub pre: Vec<HookConfig>,
    /// Post-build hooks (run after build completion).
    pub post: Vec<HookConfig>,
}

impl HooksConfig {
    /// Validate hooks configuration.
    pub fn validate(&self, _diag: &mut ConfigDiagnostics) {}
}

/// Configuration for a single build hook
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookConfig {
    /// Whether this hook is enabled (default: true).
    #[serde(default = "default_enable")]
    pub enable: bool,

    /// Display name for logging (defaults to command[0]).
    pub name: Option<String>,

    /// Command and arguments to execute from the site root.
    pub command: Vec<String>,

    /// Watch mode for serve (re-execute on file changes).
    #[serde(default)]
    pub watch: WatchMode,

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

/// Watch mode for hooks in serve mode
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WatchMode {
    /// Disabled (default).
    #[default]
    #[serde(skip)]
    Disabled,
    /// Boolean: true = always re-execute, false = disabled.
    Bool(bool),
    /// Literal file/dir names: re-execute when matching files change.
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
    /// - `Patterns(paths)`: literal file/dir names matched against path relative to site root
    pub fn matches(&self, path: &std::path::Path, root: &std::path::Path) -> bool {
        match self {
            WatchMode::Disabled => false,
            WatchMode::Bool(b) => *b,
            WatchMode::Patterns(patterns) => {
                // Get relative path from root
                let rel_path = path.strip_prefix(root).unwrap_or(path);
                let rel_str = rel_path.to_string_lossy().replace('\\', "/");

                patterns
                    .iter()
                    .any(|pattern| Self::match_pattern(pattern, &rel_str))
            }
        }
    }

    fn match_pattern(pattern: &str, rel_path: &str) -> bool {
        let pattern = pattern.trim().replace('\\', "/");
        if pattern.is_empty() {
            return false;
        }

        let rel_path = rel_path.trim_start_matches("./");
        let pattern = pattern
            .trim_start_matches("./")
            .trim_start_matches('/')
            .trim_end_matches('/');
        if pattern.is_empty() {
            return false;
        }

        // Exact path match from root-relative path
        if rel_path == pattern {
            return true;
        }

        // Directory prefix match from root-relative path
        if rel_path.starts_with(&format!("{}/", pattern)) {
            return true;
        }

        // Patterns also match by basename for convenience.
        if rel_path
            .rsplit('/')
            .next()
            .is_some_and(|name| name == pattern)
        {
            return true;
        }

        false
    }

    /// Literal root-relative watch paths declared by this mode.
    pub fn path_patterns(&self) -> &[String] {
        match self {
            WatchMode::Patterns(patterns) => patterns,
            WatchMode::Disabled | WatchMode::Bool(_) => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_parse_config;

    #[test]
    fn test_watch_matches_directory_pattern() {
        let watch = WatchMode::Patterns(vec!["assets/icons".into()]);
        let root = std::path::Path::new("/site");

        assert!(watch.matches(std::path::Path::new("/site/assets/icons/a.svg"), root));
        assert!(watch.matches(
            std::path::Path::new("/site/assets/icons/nested/b.svg"),
            root
        ));
        assert!(!watch.matches(std::path::Path::new("/site/assets/images/a.svg"), root));
    }

    #[test]
    fn test_watch_matches_basename_pattern() {
        let watch = WatchMode::Patterns(vec!["app.css".into()]);
        let root = std::path::Path::new("/site");

        assert!(watch.matches(std::path::Path::new("/site/assets/styles/app.css"), root));
        assert!(!watch.matches(std::path::Path::new("/site/assets/styles/other.css"), root));
    }

    #[test]
    fn hook_watch_patterns_are_not_path_validated() {
        let config = test_parse_config(
            r#"
[[build.hooks.pre]]
command = ["echo", "bad"]
watch = ["/src", "../templates"]
"#,
        );
        let mut diag = ConfigDiagnostics::new();

        config.build.hooks.validate(&mut diag);

        assert!(diag.is_empty());
    }

    #[test]
    fn watch_leading_slash_matches_root_relative_pattern() {
        let watch = WatchMode::Patterns(vec!["/src".into()]);
        let root = std::path::Path::new("/site");

        assert!(watch.matches(std::path::Path::new("/site/src/app.ts"), root));
    }
}
