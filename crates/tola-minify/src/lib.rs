//! Deterministic minification of CSS and JavaScript source text.
//!
//! ```
//! use std::path::Path;
//! use tola_minify::{MinifiedLanguages, MinifyLanguage};
//!
//! let enabled = MinifiedLanguages::new(true, false);
//! let css = MinifyLanguage::for_source(Path::new("assets/app.css"), enabled).unwrap();
//! assert_eq!(
//!     css.minify(".a { color: red; } .b { color: red; }").unwrap(),
//!     ".a,.b{color:red}"
//! );
//!
//! assert!(MinifyLanguage::for_source(Path::new("assets/app.min.css"), enabled).is_none());
//! assert!(MinifyLanguage::for_source(Path::new("assets/app.js"), enabled).is_none());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod css;
mod js;
mod languages;

pub use css::{CssMinifyError, minify_css};
pub use js::{JsKind, JsMinifyError, minify_js};
pub use languages::{MinifiedLanguages, MinifyError, MinifyLanguage};
