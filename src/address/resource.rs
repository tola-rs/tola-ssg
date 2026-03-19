//! Resource types for the address space.
//!
//! This module defines the core types that represent addressable resources
//! in the site: pages and assets.

use std::path::Path;

use crate::asset::AssetRoute;
use crate::core::UrlPath;
use crate::page::PageRoute;

/// A resource in the site's address space
#[derive(Debug, Clone)]
pub enum Resource {
    /// A page with HTML output.
    Page {
        /// Route information (source -> output mapping)
        route: PageRoute,
        /// Page title (for diagnostics)
        title: Option<String>,
    },
    /// A static asset.
    Asset {
        /// Route information (source -> output mapping)
        route: AssetRoute,
    },
}

impl Resource {
    /// Get the URL for this resource.
    pub fn url(&self) -> &UrlPath {
        match self {
            Resource::Page { route, .. } => &route.permalink,
            Resource::Asset { route } => &route.url,
        }
    }

    /// Get the source path for this resource.
    pub fn source(&self) -> &Path {
        match self {
            Resource::Page { route, .. } => &route.source,
            Resource::Asset { route } => &route.source,
        }
    }

    /// Check if this is a page.
    pub const fn is_page(&self) -> bool {
        matches!(self, Resource::Page { .. })
    }

    /// Check if this is an asset.
    pub const fn is_asset(&self) -> bool {
        matches!(self, Resource::Asset { .. })
    }
}
