//! Asset route: source → URL → output mapping.

use std::path::PathBuf;

use crate::core::UrlPath;

use super::AssetKind;

/// Route information for a static asset.
///
/// This is the single source of truth for asset path mapping.
/// Used by both scanning and address space registration.
#[derive(Debug, Clone)]
pub struct AssetRoute {
    /// Source file path (absolute)
    pub source: PathBuf,
    /// URL path (e.g., "/assets/logo.png" or "/posts/hello/image.png")
    pub url: UrlPath,
    /// Output file path (absolute)
    pub output: PathBuf,
    /// Asset kind (Global or Colocated)
    pub kind: AssetKind,
}
