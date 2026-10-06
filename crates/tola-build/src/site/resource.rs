//! HTML documents and assets registered in the site's address space.

use std::path::PathBuf;

use tola_address::{OutputPath, UrlPath};

#[derive(Debug, Clone)]
pub struct HtmlPage {
    pub permalink: UrlPath,
    pub output: OutputPath,
    pub properties: typst::model::DocumentInfo,
    /// Sources represented in the original document body, including shared templates.
    pub sources: Vec<typst::syntax::FileId>,
}

/// An asset URL with optional configured-source identity and producer-declared response semantics.
#[derive(Debug, Clone)]
pub struct SiteAssetRoute {
    pub source: Option<PathBuf>,
    pub url: UrlPath,
    pub(crate) declaration: crate::output::semantics::OutputDeclaration,
}

impl SiteAssetRoute {
    pub const fn declaration(&self) -> &crate::output::semantics::OutputDeclaration {
        &self.declaration
    }
}

/// A resource in the site's address space.
#[derive(Debug, Clone)]
pub enum Resource {
    Page { document: HtmlPage },
    Asset { route: SiteAssetRoute },
}

impl Resource {
    pub const fn is_page(&self) -> bool {
        matches!(self, Resource::Page { .. })
    }
}
