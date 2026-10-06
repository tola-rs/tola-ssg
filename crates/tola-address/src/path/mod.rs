//! Logical output paths, URL paths, and their shared portable component rules.

mod output;
mod portable;
mod url;

pub use output::{OutputPath, OutputPathError, RESERVED_ROOT};
pub use output::{
    asset_directory_index_alias, asset_output_from_url, asset_url_from_output,
    portable_key_is_reserved, route_for_output,
};
pub use portable::PortablePathError;
pub use portable::{portable_collision_key, portable_key_is_below, portable_keys_overlap};
pub use url::{SiteReference, UrlPath, UrlPathError, browser_location};
