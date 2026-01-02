//! Asset kind definitions.

/// Kind of static asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// Global asset from assets/ directory.
    Global,
    /// Colocated asset alongside a content file.
    Colocated,
}
