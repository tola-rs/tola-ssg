//! Configuration error types.

use super::FieldPath;
use owo_colors::OwoColorize;
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

// ============================================================================
// ConfigError
// ============================================================================

/// Configuration-related errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error when reading `{0}`")]
    Io(PathBuf, #[source] std::io::Error),

    #[error("Config file parsing error")]
    Toml(#[from] toml::de::Error),

    #[error("Config validation error: {0}")]
    Validation(String),

    // NOTE: No #[from] here - we don't want source() which causes duplicate output
    #[error("{0}")]
    Diagnostics(ConfigDiagnostics),
}

// ============================================================================
// ConfigDiagnostic
// ============================================================================

/// A single configuration diagnostic
#[derive(Debug, Clone)]
pub struct ConfigDiagnostic {
    /// Config field path (e.g., "build.css.processor.input")
    pub field: FieldPath,
    /// Error description
    pub message: String,
    /// Fix hint (optional)
    pub hint: Option<String>,
}

impl ConfigDiagnostic {
    pub fn new(field: FieldPath, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Field path in cyan brackets
        writeln!(
            f,
            "{}{}{}",
            "[".dimmed(),
            self.field.as_str().cyan(),
            "]".dimmed()
        )?;
        // Error message with red bullet
        write!(f, "{} {}", "→".red(), self.message)?;
        // Hint in yellow
        if let Some(hint) = &self.hint {
            write!(f, "\n  {} {}", "hint:".yellow(), hint)?;
        }
        Ok(())
    }
}

// ============================================================================
// ConfigDiagnostics
// ============================================================================

#[derive(Debug, Default)]
pub struct ConfigDiagnostics {
    errors: Vec<ConfigDiagnostic>,
    /// Collected hints (experimental fields/sections).
    hints: Vec<FieldPath>,
    /// Collected warnings (deprecated fields).
    warnings: Vec<(FieldPath, String)>,
    /// Suppress experimental feature hints.
    pub allow_experimental: bool,
}

impl ConfigDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with allow_experimental flag.
    pub fn with_allow_experimental(allow_experimental: bool) -> Self {
        Self {
            errors: Vec::new(),
            hints: Vec::new(),
            warnings: Vec::new(),
            allow_experimental,
        }
    }

    pub fn error(&mut self, field: FieldPath, message: impl Into<String>) {
        self.errors.push(ConfigDiagnostic::new(field, message));
    }

    /// Add an error with a hint.
    pub fn error_with_hint(
        &mut self,
        field: FieldPath,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.errors
            .push(ConfigDiagnostic::new(field, message).with_hint(hint));
    }

    /// Add a warning (deprecated fields, collected for batch display).
    pub fn warn(&mut self, field: FieldPath, message: impl Into<String>) {
        self.warnings.push((field, message.into()));
    }

    /// Add a hint for experimental fields (collected for batch display).
    pub fn experimental_hint(&mut self, field: FieldPath) {
        self.hints.push(field);
    }

    /// Add a general hint (printed immediately).
    pub fn hint(&mut self, field: FieldPath, message: impl Into<String>) {
        crate::log!("hint"; "[{}] {}", field.as_str(), message.into());
    }

    /// Print collected hints and warnings in a grouped format.
    ///
    /// Call this after validation to display all hints/warnings at once.
    pub fn print_hints_and_warnings(&self) {
        if self.warnings.is_empty() && self.hints.is_empty() {
            return;
        }

        // Print warnings (deprecated fields/sections)
        if !self.warnings.is_empty() {
            crate::log!("warning"; "deprecated fields or sections, will be removed in a future version:");
            for (field, _) in &self.warnings {
                eprintln!("- {}", field.as_str());
            }
        }

        // Print hints (experimental fields/sections)
        if !self.hints.is_empty() {
            crate::log!("hint"; "experimental fields or sections, may change or be removed:");
            for field in &self.hints {
                eprintln!("- {}", field.as_str());
            }
        }
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn len(&self) -> usize {
        self.errors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn errors(&self) -> &[ConfigDiagnostic] {
        &self.errors
    }

    /// Convert to Result (returns Err if there are errors).
    pub fn into_result(self) -> Result<(), Self> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl fmt::Display for ConfigDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}\n", "config validation failed:".red().bold())?;
        for (i, err) in self.errors.iter().enumerate() {
            write!(f, "{err}")?;
            if i + 1 < self.errors.len() {
                writeln!(f, "\n")?;
            }
        }
        if self.errors.len() > 1 {
            write!(
                f,
                "\n\n{} {} {}",
                "found".dimmed(),
                self.errors.len().to_string().red().bold(),
                "errors".dimmed()
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigDiagnostics {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn test_config_error_display() {
        let io_err = ConfigError::Io(
            PathBuf::from("test.toml"),
            Error::new(ErrorKind::NotFound, "file not found"),
        );
        let display = format!("{io_err}");
        assert!(display.contains("IO error"));
        assert!(display.contains("test.toml"));

        let validation_err = ConfigError::Validation("Test validation error".to_string());
        let display = format!("{validation_err}");
        assert!(display.contains("Test validation error"));
    }
}
