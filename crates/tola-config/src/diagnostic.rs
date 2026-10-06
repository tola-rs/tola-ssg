//! Configuration error types.

use super::{ConfigPresence, FieldPath};
use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

/// Why a configuration file is not a source the current build reads.
///
/// The reason decides what the site author changes, so it stays a value until the diagnostic a
/// reader sees is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSourceRefusal {
    /// The file resolves into `.tola` or `.tola-build.lock` rather than a source file.
    GeneratedState,
    /// The file resolves outside the site root.
    OutsideSite,
}

impl ConfigSourceRefusal {
    /// What the author must change, in their own words.
    pub fn reason(self) -> &'static str {
        match self {
            Self::GeneratedState => "it resolves into `.tola` or `.tola-build.lock`",
            Self::OutsideSite => "it resolves outside the site",
        }
    }
}

impl fmt::Display for ConfigSourceRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read the configuration file")]
    Io(PathBuf, #[source] std::io::Error),

    /// The configuration file resolved to a path this build refuses to read.
    ///
    /// The refusal has the boundary's own reason so the rendered diagnostic tells the author
    /// which of the two situations they are in instead of one sentence covering both.
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

    /// Keys the configuration schema does not know, as the document spells them.
    #[error("unknown configuration fields: {}", fields.join(", "))]
    UnknownFields { fields: Vec<String> },

    #[error("{0}")]
    Validation(String),

    // No #[from]: exposing diagnostics as source() would print them twice.
    #[error("{0}")]
    Diagnostics(ConfigDiagnostics),
}

/// Severity of a configuration diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticSeverity {
    /// A validation failure that prevents the configuration from being used.
    Error,
    /// A non-fatal configuration issue.
    Warning,
}

/// Optional lifecycle classification for a configuration warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticTag {
    Experimental,
    Deprecated,
}

/// A configuration error or warning.
#[derive(Debug, Clone)]
pub struct ConfigDiagnostic {
    severity: ConfigDiagnosticSeverity,
    tag: Option<ConfigDiagnosticTag>,
    /// Config field path (e.g. `icons.collections`).
    pub field: FieldPath,
    /// The array-table elements this diagnostic was raised inside, outermost first: each array's
    /// declared field path and the element's decimal index.
    ///
    /// The field path above names every element of its array at once, so these indices are what
    /// tell the entry an author has to change apart from its siblings.
    elements: Vec<(&'static str, usize)>,
    pub message: String,
    /// User-facing help for correcting the configuration.
    pub help: Option<String>,
}

impl ConfigDiagnostic {
    pub fn new(field: FieldPath, message: impl Into<String>) -> Self {
        Self::with_severity(ConfigDiagnosticSeverity::Error, field, message)
    }

    fn with_severity(
        severity: ConfigDiagnosticSeverity,
        field: FieldPath,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            tag: None,
            field,
            elements: Vec::new(),
            message: message.into(),
            help: None,
        }
    }

    pub fn warning(field: FieldPath, message: impl Into<String>) -> Self {
        Self::with_severity(ConfigDiagnosticSeverity::Warning, field, message)
    }

    pub fn experimental(field: FieldPath) -> Self {
        Self::lifecycle_warning(
            field,
            "experimental field or section",
            ConfigDiagnosticTag::Experimental,
        )
    }

    pub fn deprecated(field: FieldPath, message: impl Into<String>) -> Self {
        Self::lifecycle_warning(field, message, ConfigDiagnosticTag::Deprecated)
    }

    fn lifecycle_warning(
        field: FieldPath,
        message: impl Into<String>,
        tag: ConfigDiagnosticTag,
    ) -> Self {
        Self {
            tag: Some(tag),
            ..Self::warning(field, message)
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

    /// Point this diagnostic at the entries of the array tables it was raised inside.
    fn in_elements(mut self, elements: Vec<(&'static str, usize)>) -> Self {
        self.elements = elements;
        self
    }

    /// The written keys this diagnostic points at, narrowest first.
    ///
    /// The field inside its own entry comes first (`build.hooks.before-build.1.command`), the
    /// entry's key follows for a field its author left out (`build.hooks.before-build.1`), and the
    /// declared field path is last: an array written in another spelling still answers, and the
    /// declared path names every entry, so it never speaks before an indexed key.
    ///
    /// An indexed key is formatted only when `elements` names one, so a diagnostic outside every
    /// array-table entry yields its declared path as a borrow.
    pub fn key_paths(&self) -> impl Iterator<Item = Cow<'_, str>> + '_ {
        let declared = self.field.as_str();
        let written = super::field::indexed_field_path(declared, &self.elements);
        let entry = self
            .entry_key()
            .filter(|entry| written.as_deref() != Some(entry.as_str()));
        written
            .into_iter()
            .map(Cow::Owned)
            .chain(entry.into_iter().map(Cow::Owned))
            .chain(std::iter::once(Cow::Borrowed(declared)))
    }

    /// The innermost entry's key, with every enclosing element's index.
    fn entry_key(&self) -> Option<String> {
        let (array, _) = self.elements.last()?;
        super::field::indexed_field_path(array, &self.elements)
    }
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "[{}]", self.field.as_str())?;
        write!(f, "-> {}", self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {}", help)?;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct ConfigDiagnostics {
    errors: Vec<ConfigDiagnostic>,
    /// Fields and sections explicitly configured with experimental status.
    experimental_fields: Vec<FieldPath>,
    warnings: Vec<ConfigDiagnostic>,
    allow_experimental: bool,
    presence: ConfigPresence,
    /// The array-table elements validation is reading, outermost first: each array's declared
    /// field path and the element's decimal index.
    array_scopes: Vec<(&'static str, usize)>,
}

impl ConfigDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a collector that optionally suppresses experimental warnings.
    pub fn with_allow_experimental(allow_experimental: bool) -> Self {
        Self {
            allow_experimental,
            ..Self::default()
        }
    }

    /// Attach explicit field/section presence information.
    pub fn set_presence(&mut self, presence: ConfigPresence) {
        self.presence = presence;
    }

    pub fn allows_experimental(&self) -> bool {
        self.allow_experimental
    }

    /// Returns true if the given field path was explicitly present in TOML.
    #[inline]
    pub fn is_present(&self, path: &str) -> bool {
        self.presence.contains_scoped(path, &self.array_scopes)
    }

    /// Check explicit TOML presence for one array-table element's field.
    #[inline]
    pub fn is_present_indexed(&self, array_path: &str, index: usize, field: &str) -> bool {
        self.presence.contains_indexed(array_path, index, field)
    }

    #[inline]
    pub fn is_present_section(&self, section: &str) -> bool {
        self.is_present(section)
    }

    /// Validate one array-table element, retaining enclosing scopes.
    ///
    /// Field paths stay declared and unindexed, as `Config` generates them; a diagnostic recorded
    /// while the element is open has its index, so it points at this entry.
    pub fn with_array_element(
        &mut self,
        array_path: &'static str,
        index: usize,
        validate: impl FnOnce(&mut Self),
    ) {
        self.array_scopes.push((array_path, index));
        validate(self);
        self.array_scopes.pop();
    }

    /// Returns true when raw TOML presence information is available.
    #[inline]
    pub fn has_presence(&self) -> bool {
        !self.presence.is_empty()
    }

    pub fn error(&mut self, field: FieldPath, message: impl Into<String>) {
        self.push_error(ConfigDiagnostic::new(field, message));
    }

    pub fn error_with_help(
        &mut self,
        field: FieldPath,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push_error(ConfigDiagnostic::new(field, message).with_help(help));
    }

    /// Record one failure, inside the array-table entry `with_array_element` opened when there
    /// is one.
    fn push_error(&mut self, diagnostic: ConfigDiagnostic) {
        self.errors
            .push(diagnostic.in_elements(self.array_scopes.clone()));
    }

    /// Record an explicitly configured experimental field.
    pub fn record_experimental_field(&mut self, field: FieldPath) {
        self.experimental_fields.push(field);
    }

    /// Add a non-fatal warning.
    pub fn warning(&mut self, field: FieldPath, message: impl Into<String>) {
        self.push_warning(ConfigDiagnostic::warning(field, message));
    }

    /// Add a non-fatal warning with the action that resolves it.
    pub fn warning_with_help(
        &mut self,
        field: FieldPath,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push_warning(ConfigDiagnostic::warning(field, message).with_help(help));
    }

    pub fn deprecated(&mut self, field: FieldPath, message: impl Into<String>) {
        self.push_warning(ConfigDiagnostic::deprecated(field, message));
    }

    /// Record one warning, inside the array-table entry `with_array_element` opened when there
    /// is one.
    fn push_warning(&mut self, diagnostic: ConfigDiagnostic) {
        self.warnings
            .push(diagnostic.in_elements(self.array_scopes.clone()));
    }

    /// Return experimental fields collected during validation.
    pub fn experimental_fields(&self) -> &[FieldPath] {
        &self.experimental_fields
    }

    pub fn warnings(&self) -> &[ConfigDiagnostic] {
        &self.warnings
    }

    pub fn errors(&self) -> &[ConfigDiagnostic] {
        &self.errors
    }

    /// Return the collected diagnostics if any errors were recorded.
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
        for (i, err) in self.errors.iter().enumerate() {
            write!(f, "{err}")?;
            if i + 1 < self.errors.len() {
                writeln!(f, "\n")?;
            }
        }
        if self.errors.len() > 1 {
            write!(f, "\n\nfound {} errors", self.errors.len())?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigDiagnostics {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_paths_name_their_own_entry() {
        let mut diagnostics = ConfigDiagnostics::new();
        diagnostics.with_array_element("groups", 0, |diagnostics| {
            diagnostics.with_array_element("groups.entries", 1, |diagnostics| {
                diagnostics.error(FieldPath::new("groups.entries.future"), "not implemented");
            });
        });

        assert_eq!(
            diagnostics.errors()[0].key_paths().collect::<Vec<_>>(),
            [
                "groups.0.entries.1.future",
                "groups.0.entries.1",
                "groups.entries.future",
            ]
        );
    }

    #[test]
    fn array_field_points_at_the_entry() {
        let mut diagnostics = ConfigDiagnostics::new();
        diagnostics.with_array_element("assets.trees", 1, |diagnostics| {
            diagnostics.error(FieldPath::new("assets.trees"), "overlaps the content root");
        });

        assert_eq!(
            diagnostics.errors()[0].key_paths().collect::<Vec<_>>(),
            ["assets.trees.1", "assets.trees"]
        );
    }

    #[test]
    fn declared_path_answers_outside_entry() {
        let mut diagnostics = ConfigDiagnostics::new();
        diagnostics.error(FieldPath::new("build.entry"), "is empty");

        assert_eq!(
            diagnostics.errors()[0].key_paths().collect::<Vec<_>>(),
            ["build.entry"]
        );
    }

    #[test]
    fn library_output_has_no_terminal_escapes() {
        let diagnostic =
            ConfigDiagnostic::new(FieldPath::new("build.publish-dir"), "must be writable")
                .with_help("Choose another directory");

        assert!(!diagnostic.to_string().contains("\u{1b}["));
    }

    #[test]
    fn severity_is_independent_of_message_text() {
        let error = ConfigDiagnostic::new(FieldPath::new("theme"), "experimental field");
        assert_eq!(error.severity(), ConfigDiagnosticSeverity::Error);

        let experimental = ConfigDiagnostic::experimental(FieldPath::new("theme"));
        assert_eq!(experimental.tag(), Some(ConfigDiagnosticTag::Experimental));
        assert_eq!(experimental.severity(), ConfigDiagnosticSeverity::Warning);

        let deprecated =
            ConfigDiagnostic::deprecated(FieldPath::new("theme"), "experimental field");
        assert_eq!(deprecated.tag(), Some(ConfigDiagnosticTag::Deprecated));
        assert_eq!(deprecated.severity(), ConfigDiagnosticSeverity::Warning);

        let warning = ConfigDiagnostic::warning(FieldPath::new("theme"), "deprecated field");
        assert_eq!(warning.tag(), None);
        assert_eq!(warning.severity(), ConfigDiagnosticSeverity::Warning);
    }
}
