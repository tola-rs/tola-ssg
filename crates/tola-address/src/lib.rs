//! Address identity for Tola sites: routes, logical output paths, and browser URLs.
//!
//! Slugs belong to `tola-slugify`; realized documents and route indexes belong to the build engine.

#![forbid(unsafe_code)]

mod browser;
mod link;
mod mount;
mod origin;
mod path;
mod route;

pub use browser::{BrowserRef, BrowserRefError, classify_browser_ref, is_local_base_href};
pub use link::{DestinationParts, LinkKind, RequiredResourceKind, split_destination};
pub use mount::{SiteUrlMount, SiteUrlMountError, browser_url};
pub use origin::{SiteOrigin, SiteOriginError};
pub use path::{
    OutputPath, OutputPathError, PortablePathError, RESERVED_ROOT, SiteRef, UrlPath, UrlPathError,
    asset_directory_index_alias, asset_output_from_url, asset_url_from_output, browser_location,
    portable_collision_key, portable_key_is_below, portable_key_is_reserved, portable_keys_overlap,
    route_for_output, validate_portable_segment,
};
pub use route::{RouteSegmentError, slugify_segments};
