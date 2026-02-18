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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::asset::AssetKind;

    #[test]
    fn test_resource_methods() {
        let route = PageRoute {
            source: PathBuf::from("test.typ"),
            permalink: UrlPath::from_page("/test/"),
            output_file: PathBuf::from("public/test/index.html"),
            is_index: false,
            is_404: false,
            colocated_dir: None,
            output_dir: PathBuf::from("public/test"),
            full_url: String::new(),
            relative: String::new(),
        };
        let page = Resource::Page {
            route,
            title: Some("Test".to_string()),
        };

        assert!(page.is_page());
        assert!(!page.is_asset());
        assert_eq!(page.url(), "/test/");
        assert_eq!(page.source(), Path::new("test.typ"));

        let asset = Resource::Asset {
            route: AssetRoute {
                source: PathBuf::from("assets/logo.png"),
                url: UrlPath::from_asset("/assets/logo.png"),
                output: PathBuf::from("public/assets/logo.png"),
                kind: AssetKind::Global,
            },
        };

        assert!(!asset.is_page());
        assert!(asset.is_asset());
        assert_eq!(asset.url(), "/assets/logo.png");
    }
}
