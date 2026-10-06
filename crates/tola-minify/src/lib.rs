//! Deterministic minification of CSS and JavaScript source text.
//!
//! [`MinifiedLanguages`] holds the languages a caller minifies, and [`MinifyRequest`] resolves one
//! source name to the minification it takes, or to none. Only the transformation and that
//! recognition live here: what a source is published as, the identity taken from its published
//! bytes, and the diagnostic a failure becomes belong to the caller.
//!
//! Legal comments (`/*! … */`) survive minification, a classic script keeps its top-level bindings
//! because inline handlers and other scripts on the page may reference them by name, and a module
//! may mangle anything it does not export.
//!
//! ```
//! use std::path::Path;
//! use tola_minify::{MinifiedLanguages, MinifyRequest};
//!
//! let languages = MinifiedLanguages::new(true, false);
//! let stylesheet = MinifyRequest::for_source(Path::new("assets/app.css"), languages).unwrap();
//! assert_eq!(
//!     stylesheet.minify(".a { color: red; } .b { color: red; }").unwrap(),
//!     ".a,.b{color:red}"
//! );
//!
//! // A source that already says it is minified, or a language the caller leaves off, is not
//! // minified.
//! assert!(MinifyRequest::for_source(Path::new("assets/app.min.css"), languages).is_none());
//! assert!(MinifyRequest::for_source(Path::new("assets/app.js"), languages).is_none());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod css;
mod javascript;
mod languages;
mod request;

pub use css::{CssMinifyError, minify_css};
pub use javascript::{JavaScriptKind, JavaScriptMinifyError, minify_javascript};
pub use languages::{MinifiedLanguages, MinifyLanguage};
pub use request::{MinifyFailure, MinifyRequest};
