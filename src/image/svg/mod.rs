//! SVG processing utilities. (WIP)
//!
//! Provides SVG optimization, format conversion, and file extraction
//! for the VDOM pipeline transform.
//!
//! # Modules
//!
//! - [`optimize`]: SVG optimization using usvg (viewBox adjustment, minification)
//! - [`convert`]: Format conversion (SVG → AVIF/PNG/WebP/JPG)
//! - [`extract`]: File extraction logic (content-hash based naming)
//! - [`bounds`]: Stroke-inclusive bounding box calculation
//!
//! # Architecture
//!
//! ```text
//! SVG content (from VDOM)
//!         │
//!         ▼
//!    ┌──────────┐
//!    │ optimize │ ──► usvg optimization + viewBox expansion
//!    └────┬─────┘
//!         │
//!         ▼
//!    ┌─────────┐
//!    │ convert │ ──► AVIF/PNG/WebP (builtin/magick/ffmpeg)
//!    └────┬────┘
//!         │
//!         ▼
//!    ┌─────────┐
//!    │ extract │ ──► Write to .tola/ with content-hash filename
//!    └─────────┘
//! ```

mod bounds;
mod convert;
mod extract;
mod optimize;

pub use extract::{extract_svg_to_file, ExtractContext};
pub use optimize::{optimize_svg, OptimizeOptions};

// Re-export types from config

/// Compute blake3 hash of SVG content (for diff comparison).
///
/// Used in `SvgIndexed.content_hash` to quickly skip unchanged SVGs during diff.
#[allow(dead_code)]
pub fn content_hash(svg_content: &[u8]) -> u64 {
    let hash = blake3::hash(svg_content);
    let bytes = hash.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// Compute blake3 hash for filename (12 hex chars).
///
/// Used for cache-correct file naming: content changes → filename changes.
pub fn filename_hash(content: &[u8]) -> String {
    let hash = blake3::hash(content);
    hash.to_hex()[..12].to_string()
}
