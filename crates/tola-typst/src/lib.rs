#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// The quarantined layer is deprecated as a whole, so its own tests and the test
// markers that name them would warn in a test build that enables it.
#![cfg_attr(all(test, feature = "legacy-serialization"), allow(deprecated))]
// Proving `Send` for the parallel bundle test closures searches `typst_bundle::Bundle`'s nested
// map deeper than the default recursion limit, which rustc reports as
// `recursion_depth_exceeding_limit`.
#![recursion_limit = "256"]

pub mod bundle;
/// JSON conversion for native Typst values and content.
#[cfg(feature = "legacy-serialization")]
#[deprecated(note = "native Typst values pass through the pipeline; scheduled for deletion")]
pub mod codegen;
pub mod compile;
pub mod diagnostic;
pub mod html;
pub mod introspection;
pub mod prelude;
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
