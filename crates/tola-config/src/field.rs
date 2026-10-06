//! Configuration field paths.

use std::{borrow::Cow, fmt};

/// A static configuration field path.
///
/// `#[derive(Config)]` generates named accessors for a struct's fields.
///
/// # Example
///
/// ```ignore
/// #[derive(Config)]
/// #[config(section = "site")]
/// pub struct SiteMetaConfig {
///     pub origin: Option<String>,
/// }
///
/// diag.error(SiteMetaConfig::FIELDS.origin, "must use http or https");
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
        write!(f, "`{}`", self.0)
    }
}

impl AsRef<str> for FieldPath {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// Insert array-table indices from the innermost element outwards.
pub(crate) fn indexed_field_path(
    field: &str,
    elements: &[(&'static str, usize)],
) -> Option<String> {
    let mut key = Cow::Borrowed(field);
    let mut inside_element = false;
    for (array, index) in elements.iter().rev() {
        let Some(rest) = key.strip_prefix(*array) else {
            continue;
        };
        key = match rest.strip_prefix('.') {
            Some(rest) => Cow::Owned(format!("{array}.{index}.{rest}")),
            None if rest.is_empty() => Cow::Owned(format!("{array}.{index}")),
            // A shared text prefix does not put a field inside the array.
            None => continue,
        };
        inside_element = true;
    }
    inside_element.then(|| key.into_owned())
}
