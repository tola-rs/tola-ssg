//! Site-specific VDOM transforms.
//!
//! Each transform operates on a single phase, enabling composition via Pipeline.
//!
//! # Modules
//!
//! - `header`: Injects `<head>` content and sets `lang` attribute (Raw → Raw)
//! - `link`: Processes href/src and heading id with slugification (Indexed → Indexed)
//! - `media`: Processes media elements with auto-enhance (Indexed → Indexed)
//! - `svg`: Processes SVG elements (optimize/extract) (Indexed → Indexed)
//! - `body`: Injects body scripts (SPA navigation) (Indexed → Indexed)

mod body;
mod header;
mod link;
mod media;
mod svg;

pub use body::BodyInjector;
pub use header::HeaderInjector;
pub use link::LinkTransform;
pub use media::{MediaTransform, cleanup_nobg_originals};
pub use svg::SvgTransform;
