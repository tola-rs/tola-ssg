//! CSS processing configuration.
//!
//! # Example
//!
//! ```toml
//! [build.css.processor]
//! enable = true
//! input = "assets/css/main.css"
//! command = ["tailwindcss"]
//! # Automatically expands to:
//! #   command = ["tailwindcss", "-i", "$TOLA_INPUT", "-o", "$TOLA_OUTPUT"]
//! #   inject = "stylesheet"
//! #   watch = true
//! #   build_args = ["--minify"]
//! ```

use crate::config::ConfigDiagnostics;
use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CssConfig {
    /// CSS processor configuration.
    pub processor: CssProcessorConfig,
}

impl CssConfig {
    /// Validate CSS configuration.
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        self.processor.validate(diag);
    }
}

/// CSS processor configuration (syntax sugar for pre hook).
///
/// When enabled, this is internally compiled to a pre hook with:
/// - Auto-expanded command with `-i $TOLA_INPUT -o $TOLA_OUTPUT`
/// - `inject = "stylesheet"`
/// - `watch = true` (auto-watch input file)
/// - `build_args = ["--minify"]`
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.css.processor")]
pub struct CssProcessorConfig {
    /// Enable CSS processor.
    pub enable: bool,
    /// Input CSS file path.
    pub input: Option<PathBuf>,
    /// CSS processor command (e.g., `["tailwindcss"]` or `["npx", "tailwindcss"]`).
    pub command: Vec<String>,
    /// Suppress output (default: true).
    pub quiet: bool,
}

impl Default for CssProcessorConfig {
    fn default() -> Self {
        Self {
            enable: false,
            input: None,
            command: vec!["tailwindcss".into()],
            quiet: true,
        }
    }
}

impl CssProcessorConfig {
    /// Validate CSS processor configuration.
    ///
    /// # Checks
    /// - If enabled:
    ///   - `command` must not be empty
    ///   - `command[0]` must be an installed executable (or package runner)
    ///   - `input` must be configured and point to an existing file
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

        // Input must be configured
        let Some(input) = &self.input else {
            diag.error(
                Self::FIELDS.input,
                format!(
                    "{} is true but {} is not configured",
                    Self::FIELDS.enable,
                    Self::FIELDS.input
                ),
            );
            return;
        };

        // Input must exist and be a file
        if !input.exists() {
            diag.error(
                Self::FIELDS.input,
                format!("{} file not found: {}", Self::FIELDS.input, input.display()),
            );
        } else if !input.is_file() {
            diag.error(
                Self::FIELDS.input,
                format!("{} is not a file: {}", Self::FIELDS.input, input.display()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_parse_config;

    #[test]
    fn test_defaults() {
        let config = test_parse_config("");
        assert!(!config.build.css.processor.enable);
        assert!(config.build.css.processor.input.is_none());
        assert_eq!(config.build.css.processor.command, vec!["tailwindcss"]);
    }

    #[test]
    fn test_processor_config() {
        let config = test_parse_config(
            r#"
[build.css.processor]
enable = true
input = "assets/styles/main.css"
command = ["tailwindcss-v4"]
"#,
        );
        assert!(config.build.css.processor.enable);
        assert_eq!(
            config.build.css.processor.input,
            Some(PathBuf::from("assets/styles/main.css"))
        );
        assert_eq!(config.build.css.processor.command, vec!["tailwindcss-v4"]);
    }

    #[test]
    fn test_processor_command_multiple_args() {
        let config =
            test_parse_config("[build.css.processor]\ncommand = [\"npx\", \"tailwindcss\"]");
        assert_eq!(
            config.build.css.processor.command,
            vec!["npx", "tailwindcss"]
        );
    }
}
