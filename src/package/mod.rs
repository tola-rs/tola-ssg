//! Virtual Package System for `@tola/*` packages.
//!
//! Provides three virtual packages:
//! - `@tola/site` - Site configuration from `[site]` in tola.toml
//! - `@tola/pages` - Page metadata and filtering utilities
//! - `@tola/current` - Current page context and navigation
//!
//! # Usage in Typst
//!
//! ```typst
//! #import "@tola/site:0.0.0": info, root
//! #import "@tola/pages:0.0.0": pages, by-tag, all-tags
//! #import "@tola/current:0.0.0": permalink, siblings, prev
//! ```

mod inject;
mod phase;
mod tola;

pub use inject::{
    build_filter_inputs_with_site, build_visible_current_context_for_source, build_visible_inputs,
    build_visible_inputs_for_source,
};
pub use phase::Phase;
pub use tola::{TolaPackage, generate_lsp_stubs, package_sentinel, read_package};
