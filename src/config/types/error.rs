//! Configuration error types.

use super::{ConfigPresence, FieldPath};
use crate::logger;
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
            logger::style_dim("["),
            logger::style_path(self.field.as_str()),
            logger::style_dim("]")
        )?;
        // Error message with red bullet
        write!(f, "{} {}", logger::style_error("→"), self.message)?;
        // Hint in yellow
        if let Some(hint) = &self.hint {
            write!(f, "\n  {} {}", logger::style_warning("hint:"), hint)?;
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
    /// Explicitly present config paths collected from raw TOML.
    presence: ConfigPresence,
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
            presence: ConfigPresence::default(),
        }
    }

    /// Attach explicit field/section presence information.
    pub fn set_presence(&mut self, presence: ConfigPresence) {
        self.presence = presence;
    }

    /// Returns true if the given field path was explicitly present in TOML.
    #[inline]
    pub fn is_present(&self, path: &str) -> bool {
        self.presence.contains(path)
    }

    /// Returns true if the given section path was explicitly present in TOML.
    #[inline]
    pub fn is_present_section(&self, section: &str) -> bool {
        self.presence.contains(section)
    }

    /// Returns true when raw TOML presence information is available.
    #[inline]
    pub fn has_presence(&self) -> bool {
        !self.presence.is_empty()
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
        logger::log(
            "hint",
            format_args!("[{}] {}", field.as_str(), message.into()),
        );
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
            let fields = self
                .warnings
                .iter()
                .map(|(field, _)| format!("- {}", field.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            logger::block(
                "warning",
                "deprecated fields or sections, will be removed in a future version:",
                &fields,
            );
        }

        // Print hints (experimental fields/sections)
        if !self.hints.is_empty() {
            let fields = self
                .hints
                .iter()
                .map(|field| format!("- {}", field.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            logger::block(
                "hint",
                "experimental fields or sections, may change or be removed:",
                &fields,
            );
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
        writeln!(
            f,
            "{}\n",
            logger::style_error_strong("config validation failed:")
        )?;
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
                logger::style_dim("found"),
                logger::style_error_strong(self.errors.len().to_string()),
                logger::style_dim("errors")
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
        assert_eq!(format!("{io_err}"), "IO error when reading `test.toml`");

        let validation_err = ConfigError::Validation("Test validation error".to_string());
        assert_eq!(
            format!("{validation_err}"),
            "Config validation error: Test validation error"
        );
    }
}
