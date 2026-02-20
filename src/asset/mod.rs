//! Asset processing and path mapping.

mod generated;
mod kind;
mod meta;
pub mod minify;
mod process;
mod route;
mod scan;
pub mod version;

// Types
pub use kind::AssetKind;
pub use route::AssetRoute;

// Scanning (pure functions)
pub use scan::{scan_content_assets, scan_flatten_assets, scan_global_assets};

// Processing (side effects)
pub use process::{
    process_asset, process_cname, process_content_assets, process_flatten_assets, process_rel_asset,
};

// Metadata helpers
pub use meta::{compute_asset_href, route_from_source, url_from_output_path};
