//! Page data collected before full compilation.

use std::path::PathBuf;

use crate::core::{LinkKind, LinkOrigin};
use crate::page::{PageKind, PageMeta};

/// Pre-scanned page data used by build and serve startup paths.
///
/// The scan phase collects metadata, page kind, links, and headings before
/// full compilation so page registries can be populated consistently.
#[derive(Debug, Clone)]
pub struct ScannedPage {
    /// Source file path.
    pub path: PathBuf,
    /// Page metadata from scan.
    pub meta: Option<PageMeta>,
    /// Page compilation kind.
    pub kind: PageKind,
    /// Internal page link candidates extracted during scan.
    pub links: Vec<ScannedPageLink>,
    /// Document headings extracted during scan.
    pub headings: Vec<ScannedHeading>,
}

/// A heading extracted from the document during scan.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScannedHeading {
    /// Heading level.
    pub level: u8,
    /// Heading text content.
    pub text: String,
    /// Heading supplement, such as a custom Typst section name.
    pub supplement: Option<String>,
}

/// A link candidate extracted during scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedPageLink {
    /// Raw link destination.
    pub dest: String,
    /// Source attribute or element kind.
    pub origin: LinkOrigin,
}

impl ScannedPageLink {
    pub fn new(dest: impl Into<String>, origin: LinkOrigin) -> Self {
        Self {
            dest: dest.into(),
            origin,
        }
    }

    /// Whether this source is known to be asset-only rather than page navigation.
    pub fn is_asset_attr(&self) -> bool {
        self.origin.is_asset_attr()
    }

    /// Keep only links that can plausibly represent page navigation.
    pub fn is_page_candidate(&self) -> bool {
        !self.is_asset_attr() && !matches!(LinkKind::parse(&self.dest), LinkKind::External(_))
    }
}

impl ScannedPage {
    /// Partition scanned pages by content kind.
    pub fn partition_by_kind(scanned: &[ScannedPage]) -> (Vec<&ScannedPage>, Vec<&ScannedPage>) {
        use crate::core::ContentKind;
        scanned
            .iter()
            .partition(|s| ContentKind::from_path(&s.path) == Some(ContentKind::Typst))
    }
}
