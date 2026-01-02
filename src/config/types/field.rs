//! Type-safe config field path.

use owo_colors::OwoColorize;
use std::fmt;

/// A type-safe wrapper for config field paths.
///
/// Used with `#[derive(Config)]` to generate compile-time checked
/// field path accessors.
///
/// # Example
///
/// ```ignore
/// #[derive(Config)]
/// #[config(section = "site")]
/// pub struct SiteMetaConfig {
///     pub url: Option<String>,
/// }
///
/// // Generated:
/// impl SiteMetaConfig {
///     pub const FIELDS: SiteMetaConfigFields = ...;
/// }
///
/// // Usage:
/// diag.error(SiteMetaConfig::FIELDS.url, "required");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldPath(pub &'static str);

impl FieldPath {
    #[inline]
    pub const fn new(path: &'static str) -> Self {
        Self(path)
    }

    #[inline]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_args!("`{}`", self.0).bright_blue())
    }
}

impl AsRef<str> for FieldPath {
    fn as_ref(&self) -> &str {
        self.0
    }
}
