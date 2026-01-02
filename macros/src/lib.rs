//! Proc macros for tola-ssg.
//!
//! # Config derive macro
//!
//! Generates both field path accessors and TOML template.
//!
//! ```ignore
//! #[derive(Config)]
//! #[config(section = "site.info")]
//! /// Site metadata configuration.
//! pub struct SiteInfoConfig {
//!     /// Site title displayed in browser tab.
//!     pub title: String,
//!
//!     /// Language code (BCP 47).
//!     #[config(default = "\"en\"")]
//!     pub language: String,
//!
//!     /// Enable dark mode.
//!     #[config(experimental)]
//!     pub dark_mode: bool,
//!
//!     /// Internal field.
//!     #[config(skip)]
//!     pub internal: String,
//! }
//!
//! // Generates:
//! // - SiteInfoConfig::FIELDS.title -> FieldPath("site.info.title")
//! // - SiteInfoConfig::template() -> TOML string with comments
//! // - SiteInfoConfig::template_with_header() -> with [section] header
//! ```
//!
//! # Attributes
//!
//! Struct-level:
//! - `#[config(section = "path")]` - TOML section path
//!
//! Field-level:
//! - `#[config(skip)]` - Skip from FIELDS (internal use)
//! - `#[config(hidden)]` - Hide from template output
//! - `#[config(name = "x")]` - Custom TOML field name
//! - `#[config(default = "x")]` - Default value in template
//! - `#[config(experimental)]` - Mark as experimental
//! - `#[config(not_implemented)]` - Mark as not implemented
//! - `#[config(deprecated)]` - Mark as deprecated
//!
//! # Section inference
//!
//! Without `section` attribute, inferred from struct name:
//! - `SiteInfoConfig` → `site_info`
//! - `CssConfig` → `css`

mod config;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

/// Derive macro that generates FIELDS and template().
#[proc_macro_derive(Config, attributes(config))]
pub fn derive_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    config::derive(&input).into()
}
