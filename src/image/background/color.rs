use image::RgbaImage;
use lab::{Lab, rgb_bytes_to_labs};

/// Pre-convert entire image to LAB color space.
///
/// Uses SIMD-accelerated batch conversion from the `lab` crate.
pub(super) fn preconvert_to_lab(img: &RgbaImage) -> Vec<Lab> {
    let mut rgb_bytes = Vec::with_capacity(img.width() as usize * img.height() as usize * 3);
    for pixel in img.pixels() {
        rgb_bytes.extend_from_slice(&pixel.0[..3]);
    }
    rgb_bytes_to_labs(&rgb_bytes)
}

/// Squared color distance in LAB space (ΔE^2).
#[inline]
pub(super) fn color_distance_sq(c1: &Lab, c2: &Lab) -> f32 {
    let dl = c1.l - c2.l;
    let da = c1.a - c2.a;
    let db = c1.b - c2.b;
    dl * dl + da * da + db * db
}
