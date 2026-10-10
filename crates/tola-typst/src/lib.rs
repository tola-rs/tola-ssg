#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Proving `Send` for the parallel bundle test closures searches `typst_bundle::Bundle`'s nested
// map deeper than the default recursion limit, which rustc reports as
// `recursion_depth_exceeding_limit`.
#![recursion_limit = "256"]

pub mod bundle;
pub mod compile;
pub mod diagnostic;
pub mod html;
pub mod introspection;
pub mod prelude;
pub mod reference_occurrence;
pub mod world;

mod extract;
mod session;

pub use prelude::*;

pub use typst;
pub use typst_bundle;
pub use typst_eval;
pub use typst_html;
pub use typst_kit;
pub use typst_layout;
pub use typst_pdf;
pub use typst_render;
pub use typst_svg;
