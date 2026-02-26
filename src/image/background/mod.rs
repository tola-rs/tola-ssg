//! Remove background from images.
//!
//! Uses LAB color space for perceptually accurate color matching,
//! with edge-seeded flood fill to remove connected background regions.

mod color;
mod detect;
mod floodfill;
mod mask;
mod process;

pub use process::remove_background;
