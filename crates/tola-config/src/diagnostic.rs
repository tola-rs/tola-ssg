//! Configuration error types.

use super::{ConfigPresence, FieldPath};
use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

/// Why a configuration file is excluded from reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSourceRefusal {
    GeneratedState,
    OutsideSite,
}

impl ConfigSourceRefusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::GeneratedState => "it resolves into `.tola` or `.tola-build.lock`",
            Self::OutsideSite => "it resolves outside the site",
        }
    }
}

impl fmt::Display for ConfigSourceRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read the configuration file")]
    Io(PathBuf, #[source] std::io::Error),

    #[error("the configuration file is not a source this build reads: {reason}")]
    Refused {
        path: PathBuf,
        reason: ConfigSourceRefusal,
    },

    #[error("this is not valid TOML")]
    Toml {
        path: PathBuf,
        input: Box<str>,
        #[source]
        source: toml::de::Error,
    },

    #[error("unknown configuration fields: {}", fields.join(", "))]
    UnknownFields { fields: Vec<String> },

    #[error("`{path}` is not a file")]
    PathNotAFile { path: String },

    // No #[from]: exposing diagnostics as source() would print them twice.
    #[error("{0}")]
    Diagnostics(ConfigDiagnostics),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticTag {
    Experimental,
    Deprecated,
}

#[derive(Debug, Clone)]
pub struct ConfigDiagnostic {
    severity: ConfigDiagnosticSeverity,
    tag: Option<ConfigDiagnosticTag>,
    pub field: FieldPath,
    elements: Vec<(&'static str, usize)>,
    pub message: String,
    pub help: Option<String>,
}

impl ConfigDiagnostic {
    pub fn new(field: FieldPath, msg: impl Into<String>) -> Self {
        Self::with_severity(ConfigDiagnosticSeverity::Error, field, msg)
    }

    fn with_severity(
        severity: ConfigDiagnosticSeverity,
        field: FieldPath,
        msg: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            tag: None,
            field,
            elements: Vec::new(),
            message: msg.into(),
            help: None,
        }
    }

    pub fn warning(field: FieldPath, msg: impl Into<String>) -> Self {
        Self::with_severity(ConfigDiagnosticSeverity::Warning, field, msg)
    }

    pub fn experimental(field: FieldPath) -> Self {
        Self::lifecycle_warning(
            field,
            "experimental field or section",
            ConfigDiagnosticTag::Experimental,
        )
    }

    pub fn deprecated(field: FieldPath, msg: impl Into<String>) -> Self {
        Self::lifecycle_warning(field, msg, ConfigDiagnosticTag::Deprecated)
    }

    fn lifecycle_warning(
        field: FieldPath,
        msg: impl Into<String>,
        tag: ConfigDiagnosticTag,
    ) -> Self {
        Self {
            tag: Some(tag),
            ..Self::warning(field, msg)
        }
    }

    pub fn severity(&self) -> ConfigDiagnosticSeverity {
        self.severity
    }

    pub fn tag(&self) -> Option<ConfigDiagnosticTag> {
        self.tag
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn in_elements(mut self, elements: Vec<(&'static str, usize)>) -> Self {
        self.elements = elements;
        self
    }

    /// Key paths ordered narrowest-first: indexed field, indexed entry, then declared field.
    pub fn key_paths(&self) -> impl Iterator<Item = Cow<'_, str>> + '_ {
        let declared = self.field.as_str();
        let written = super::field::indexed_field_path(declared, &self.elements);
        let entry = self
            .elements
            .last()
            .and_then(|(array, _)| super::field::indexed_field_path(array, &self.elements))
            .filter(|entry| written.as_deref() != Some(entry.as_str()));
        written
            .into_iter()
            .map(Cow::Owned)
            .chain(entry.into_iter().map(Cow::Owned))
            .chain(std::iter::once(Cow::Borrowed(declared)))
    }
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "[{}]", self.field.as_str())?;
        write!(f, "-> {}", self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, thiserror::Error)]
pub struct ConfigDiagnostics {
    errors: Vec<ConfigDiagnostic>,
    experimental_fields: Vec<FieldPath>,
    warnings: Vec<ConfigDiagnostic>,
    allow_experimental: bool,
    presence: ConfigPresence,
    array_scopes: Vec<(&'static str, usize)>,
}

impl ConfigDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_allow_experimental(allow_experimental: bool) -> Self {
        Self {
            allow_experimental,
            ..Self::default()
        }
    }

    pub fn set_presence(&mut self, presence: ConfigPresence) {
        self.presence = presence;
    }

    pub fn allows_experimental(&self) -> bool {
        self.allow_experimental
    }

    #[inline]
    pub fn is_present(&self, path: &str) -> bool {
        self.presence.contains_scoped(path, &self.array_scopes)
    }

    #[inline]
    pub fn is_present_indexed(&self, array_path: &str, idx: usize, field: &str) -> bool {
        self.presence.contains_indexed(array_path, idx, field)
    }

    #[inline]
    pub fn is_present_section(&self, section: &str) -> bool {
        self.is_present(section)
    }

    pub fn with_array_element(
        &mut self,
        array_path: &'static str,
        idx: usize,
        validate: impl FnOnce(&mut Self),
    ) {
        self.array_scopes.push((array_path, idx));
        validate(self);
        self.array_scopes.pop();
    }

    #[inline]
    pub fn has_presence(&self) -> bool {
        !self.presence.is_empty()
    }

    pub fn error(&mut self, field: FieldPath, msg: impl Into<String>) {
        self.push_error(ConfigDiagnostic::new(field, msg));
    }

    pub fn error_with_help(
        &mut self,
        field: FieldPath,
        msg: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push_error(ConfigDiagnostic::new(field, msg).with_help(help));
    }

    fn push_error(&mut self, diag: ConfigDiagnostic) {
        self.errors
            .push(diag.in_elements(self.array_scopes.clone()));
    }

    pub fn record_experimental_field(&mut self, field: FieldPath) {
        self.experimental_fields.push(field);
    }

    pub fn warning(&mut self, field: FieldPath, msg: impl Into<String>) {
        self.push_warning(ConfigDiagnostic::warning(field, msg));
    }

    pub fn warning_with_help(
        &mut self,
        field: FieldPath,
        msg: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push_warning(ConfigDiagnostic::warning(field, msg).with_help(help));
    }

    pub fn deprecated(&mut self, field: FieldPath, msg: impl Into<String>) {
        self.push_warning(ConfigDiagnostic::deprecated(field, msg));
    }

    fn push_warning(&mut self, diag: ConfigDiagnostic) {
        self.warnings
            .push(diag.in_elements(self.array_scopes.clone()));
    }

    pub fn experimental_fields(&self) -> &[FieldPath] {
        &self.experimental_fields
    }

    pub fn warnings(&self) -> &[ConfigDiagnostic] {
        &self.warnings
    }

    pub fn errors(&self) -> &[ConfigDiagnostic] {
        &self.errors
    }

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
        writeln!(f, "config validation failed:\n")?;
        for (idx, err) in self.errors.iter().enumerate() {
            write!(f, "{err}")?;
            if idx + 1 < self.errors.len() {
                writeln!(f, "\n")?;
            }
        }
        if self.errors.len() > 1 {
            write!(f, "\n\nfound {} errors", self.errors.len())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_paths_nested() {
        let mut diag = ConfigDiagnostics::new();
        diag.with_array_element("groups", 0, |diag| {
            diag.with_array_element("groups.entries", 1, |diag| {
                diag.error(FieldPath::new("groups.entries.future"), "not implemented");
            });
        });

        assert_eq!(
            diag.errors()[0].key_paths().collect::<Vec<_>>(),
            [
                "groups.0.entries.1.future",
                "groups.0.entries.1",
                "groups.entries.future",
            ]
        );
    }

    #[test]
    fn element_paths_array_field() {
        let mut diag = ConfigDiagnostics::new();
        diag.with_array_element("assets.trees", 1, |diag| {
            diag.error(FieldPath::new("assets.trees"), "overlaps the content root");
        });

        assert_eq!(
            diag.errors()[0].key_paths().collect::<Vec<_>>(),
            ["assets.trees.1", "assets.trees"]
        );
    }

    #[test]
    fn element_paths_flat() {
        let mut diag = ConfigDiagnostics::new();
        diag.error(FieldPath::new("build.entry"), "is empty");

        assert_eq!(
            diag.errors()[0].key_paths().collect::<Vec<_>>(),
            ["build.entry"]
        );
    }

    #[test]
    fn constructor_severities() {
        let err = ConfigDiagnostic::new(FieldPath::new("theme"), "experimental field");
        assert_eq!(err.severity(), ConfigDiagnosticSeverity::Error);

        let exp = ConfigDiagnostic::experimental(FieldPath::new("theme"));
        assert_eq!(exp.tag(), Some(ConfigDiagnosticTag::Experimental));
        assert_eq!(exp.severity(), ConfigDiagnosticSeverity::Warning);

        let dep = ConfigDiagnostic::deprecated(FieldPath::new("theme"), "experimental field");
        assert_eq!(dep.tag(), Some(ConfigDiagnosticTag::Deprecated));
        assert_eq!(dep.severity(), ConfigDiagnosticSeverity::Warning);

        let warn = ConfigDiagnostic::warning(FieldPath::new("theme"), "deprecated field");
        assert_eq!(warn.tag(), None);
        assert_eq!(warn.severity(), ConfigDiagnosticSeverity::Warning);
    }
}
