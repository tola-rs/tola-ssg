//! Page compilation kind.

use crate::package::TolaPackage;

/// Page compilation kind.
///
/// Determines whether a page needs iterative compilation to resolve
/// self-referencing metadata (e.g., `pages().len()` in metadata).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageKind {
    /// Direct compilation: does not depend on site data, compiles once.
    ///
    /// Pages that don't import `@tola/xxx`.
    #[default]
    Direct,
    /// Iterative compilation: depends on site data, may need multiple
    /// compilations to converge.
    ///
    /// Pages that import `@tola/xxx`.
    Iterative,
}

impl PageKind {
    /// Check if this page needs iterative compilation.
    #[inline]
    pub fn is_iterative(&self) -> bool {
        matches!(self, Self::Iterative)
    }

    /// Check if this page needs direct compilation.
    #[inline]
    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct)
    }

    /// Determine page kind from accessed packages.
    ///
    /// Returns `Iterative` if any package requires iteration (e.g., `@tola/pages`, `@tola/current`).
    pub fn from_packages(packages: &[typst_batch::PackageId]) -> Self {
        if packages.iter()
            .filter_map(TolaPackage::from_id)
            .any(|p| p.requires_iteration())
        {
            Self::Iterative
        } else {
            Self::Direct
        }
    }
}
