//! Proc macros for tola-config.
//!
//! # Config derive macro
//!
//! Generates field path accessors and fallible TOML template methods.
//!
//! ```ignore
//! #[derive(Config)]
//! #[config(section = "site")]
//! /// Site metadata configuration.
//! pub struct SiteConfig {
//!     /// Site title displayed in browser tab.
//!     pub title: String,
//!
//!     /// Language code (BCP 47).
//!     #[config(default = "en")]
//!     pub language: String,
//!
//!     /// Enable dark mode.
//!     #[config(status = experimental)]
//!     pub dark_mode: bool,
//!
//!     /// Internal field.
//!     #[config(skip)]
//!     pub internal: String,
//! }
//!
//! // Generates:
//! // - SiteConfig::FIELDS.title -> FieldPath("site.title")
//! // - SiteConfig::try_template() -> fallible TOML template generation
//! // - SiteConfig::try_template_with_header() -> with [section] header
//! // - SiteConfig::try_template_with_header_from(&value) -> render an explicit value
//! ```
//!
//! Every `try_template*` method returns `Result<String, ConfigTemplateError>` from the selected
//! runtime crate. A failure retains its canonical field path, every enclosing array-table index,
//! and the serializer's error chain.
//!
//! # Attributes
//!
//! Struct-level:
//! - `#[config(section = "path")]` - TOML section path
//! - `#[config(crate = tola_config)]` - override the default `::tola_config` runtime path
//! - `#[tola_config]` - Tola shorthand for `#[config(crate = tola_config)]`
//! - `#[config(collection = array_table)]` - render the section as a TOML array-of-tables
//!
//! Field-level:
//! - `#[config(skip)]` - Omit from field paths, templates, and status validation
//! - `#[config(hidden)]` - Hide from template output
//! - `#[config(name = "x")]` - Custom TOML field name (must match `serde(rename = "x")`)
//! - `#[config(default = "x")]` - Template-only scalar override; composite defaults come from `Default`
//! - `#[config(values = Type::VALUES)]` - a `&'static [&'static str]` of TOML literals offered as field values
//! - `#[config(collection = inline)]` - render a collection field as an inline TOML array
//! - `#[config(collection = array_table)]` on a `#[config(sub)]` field - render each `Vec<T>` element as a table
//! - `#[config(status = experimental)]` - Mark as experimental
//! - `#[config(status = not_implemented)]` - Mark as not implemented
//! - `#[config(status = deprecated)]` - Mark as deprecated
//!
//! A `#[config(sub)]` field writes a section, so that section's own declaration documents the
//! key: the doc comment belongs on the struct the field names, not on the key that opens it
//! the field is refused rather than kept as a second copy.
//!
//! Renamed keys require explicit serde renames; `serde(rename_all = ...)` is
//! rejected. Short and directional renames are supported, but serialization and
//! deserialization names must match. An omitted direction uses the Rust field name.
//! All canonical field keys and section path components, including inferred names, must be
//! non-empty TOML bare keys: ASCII letters, digits, underscores or hyphens. An empty whole
//! section path names the root table; punctuation is refused rather than implicitly escaped.
//! Numeric defaults must be TOML literals of the field's numeric category; string
//! defaults are unescaped values, not prequoted TOML.
//!
//! # Section inference
//!
//! Without `section` attribute, inferred from struct name:
//! - `SiteConfig` -> `site`
//! - `CssConfig` -> `css`

mod config;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Derive macro that generates field paths and fallible template methods.
#[proc_macro_derive(Config, attributes(config, tola_config))]
pub fn derive_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    config::derive(&input).into()
}
