#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod colours;
pub mod continuation;
pub mod docs;
pub mod edit;
pub mod folds;
#[cfg(feature = "format")]
pub mod format;
pub mod imports;
pub mod loops;
pub mod names;
pub mod nesting;
pub mod outline;
pub mod position;
pub mod sections;
pub mod syntax;
pub mod tokens;
pub mod usage;
pub mod wraps;

pub use typst_library;
pub use typst_syntax;
