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
//! #import "@tola/site:0.0.0": title, author, extra
//! #import "@tola/pages:0.0.0": pages, by-tag, all-tags
//! #import "@tola/current:0.0.0": path, siblings, find-prev
//! ```

mod phase;
mod tola;

pub use phase::Phase;
pub use tola::{generate_lsp_stubs, package_sentinel, read_package, TolaPackage};
