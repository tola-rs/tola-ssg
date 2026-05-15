//! Asset processing and path mapping.

mod generated;
mod kind;
pub mod minify;
mod process;
mod route;
mod scan;
pub mod version;

// Types
pub use kind::AssetKind;
pub use route::{
    AssetRoute, SYSTEM_ASSET_DIR, asset_source_hint, asset_url_exists, asset_url_hint,
    href_for_route, is_asset_url, resolve_asset_href, route_from_config_source, route_from_source,
    source_for_asset_url,
};

// Scanning (pure functions)
pub use scan::{scan_content_assets, scan_flatten_assets, scan_nested_assets};

// Processing (side effects)
pub use process::{process_asset, process_cname, process_configured_assets};
