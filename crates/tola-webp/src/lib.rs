//! A pure-Rust, deterministic still-image WebP encoder for `tola-image`.
//!
//! The encoder writes lossy VP8 still images with intra mode search, trellis quantization,
//! integer SIMD transforms, and VP8L-compressed alpha. Within one encoder revision, the same
//! pixels and quality produce the same bytes with scalar or SIMD code, including wasm.
//!
//! The public API is experimental. `tola-image` keeps libwebp as its native default and delegates
//! whole-image lossless WebP to `image-webp`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub use encoder::Error;

/// Coefficient-trellis effort for [`encode_rgba_with_mode`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrellisMode {
    /// Exhaustive trellis over the `0..=2047` magnitude domain (default).
    #[default]
    Exact,
    /// Two-candidate restricted trellis: faster, with measured rate-distortion differences.
    Restricted,
}

/// Encode straight-alpha RGBA8 pixels as one lossy WebP still image.
///
/// `quality` is `0..=100`; higher keeps more detail. The image must be at most 16,383 pixels on
/// either side.
pub fn encode_rgba(rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, Error> {
    encoder::encode_rgba(rgba, width, height, quality)
}

/// Encode straight-alpha RGBA8 pixels with an explicit trellis effort mode.
///
/// [`encode_rgba`] uses [`TrellisMode::Restricted`]; [`TrellisMode::Exact`] is the
/// quality mode with the same API.
pub fn encode_rgba_with_mode(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    mode: TrellisMode,
) -> Result<Vec<u8>, Error> {
    encoder::encode_rgba_with_mode(rgba, width, height, quality, mode)
}

mod alpha;
mod bool_writer;
mod color;
mod container;
mod cost;
mod encoder;
mod macroblock;
mod predict;
mod quantize;
mod reconstruction;
mod tables;
mod token;
mod transform;
mod trellis;
mod vp8;

#[cfg(test)]
mod tests {
    use super::encode_rgba;

    /// The same image encodes to the same bytes, so a rebuild never churns an unchanged asset.
    #[test]
    fn encoding_is_deterministic() {
        let mut rgba = Vec::new();
        for y in 0..32u8 {
            for x in 0..32u8 {
                rgba.extend_from_slice(&[x * 8, y * 8, 96, 255]);
            }
        }
        assert_eq!(
            encode_rgba(&rgba, 32, 32, 75).unwrap(),
            encode_rgba(&rgba, 32, 32, 75).unwrap()
        );
    }

    fn alpha_payload(webp: &[u8]) -> Option<&[u8]> {
        let mut position = 12;
        while position < webp.len() {
            let length =
                u32::from_le_bytes(webp[position + 4..position + 8].try_into().unwrap()) as usize;
            let payload = &webp[position + 8..position + 8 + length];
            if &webp[position..position + 4] == b"ALPH" {
                return Some(payload);
            }
            position += 8 + length + (length & 1);
        }
        None
    }

    #[test]
    fn opaque_images_omit_alpha() {
        for (width, height) in [(1, 1), (1, 17), (17, 1), (17, 19)] {
            let rgba = [37, 123, 211, 255].repeat((width * height) as usize);
            let webp = encode_rgba(&rgba, width, height, 75).unwrap();
            assert!(alpha_payload(&webp).is_none(), "{width}x{height}");
            let decoded = image::load_from_memory(&webp).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (width, height));
            assert!(decoded.pixels().all(|pixel| pixel[3] == 255));
        }
    }
}
