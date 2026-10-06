//! Address identity for Tola sites: routes, logical output paths, and browser URLs.
//!
//! A slug, a route, a logical output path, a site URL, and a browser URL are separate
//! identities, and each has one owner here. Turning text into a slug
//! belongs to `tola-slugify`; realized documents and route indexes belong to the site
//! construction engine.

#![forbid(unsafe_code)]

mod browser;
mod link;
mod mount;
mod origin;
mod path;
mod route;

pub use browser::{
    BrowserReferenceError, ResolvedBrowserReference, base_href_keeps_build_origin,
    resolve_browser_reference,
};
pub use link::{DestinationParts, LinkKind, RequiredResourceKind, split_destination};
pub use mount::{SiteUrlMount, SiteUrlMountError, browser_url};
pub use origin::{SiteOrigin, SiteOriginError};
pub use path::{
    OutputPath, OutputPathError, PortablePathError, RESERVED_ROOT, SiteReference, UrlPath,
    UrlPathError, asset_directory_index_alias, asset_output_from_url, asset_url_from_output,
    browser_location, portable_collision_key, portable_key_is_below, portable_key_is_reserved,
    portable_keys_overlap, route_for_output,
};
pub use route::{RouteSegmentError, slugify_segments};

/// Values the crate's test modules construct more than once.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::UrlPath;

    /// The decoded path one valid site-root URL text names.
    pub(crate) fn url_path(raw: &str) -> UrlPath {
        UrlPath::parse(raw).expect("test URL path is valid")
    }
}
