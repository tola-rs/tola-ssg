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
    use super::{TrellisMode, encode_rgba};

    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    #[test]
    fn encoded_bytes_are_pinned() {
        let mut rgba = Vec::new();
        for y in 0..32u8 {
            for x in 0..32u8 {
                rgba.extend_from_slice(&[x * 8, y * 8, 96, 255]);
            }
        }
        let webp = encode_rgba(&rgba, 32, 32, 75).unwrap();
        assert_eq!(webp.len(), 134);
        assert_eq!(fnv1a(&webp), 0xea01e20491288399);
        assert_eq!(webp, encode_rgba(&rgba, 32, 32, 75).unwrap());
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
    fn alpha_border_sizes_round_trip() {
        type Alpha = fn(u32, u32) -> u8;
        let cases: [(u32, u32, Alpha); 8] = [
            (1, 1, |_, _| 0),
            (1, 19, |_, y| (y * 255 / 18) as u8),
            (19, 1, |x, _| ((x * 73 + 19) % 256) as u8),
            (3, 5, |x, y| ((x * 83 + y * 47) % 256) as u8),
            (15, 17, |x, y| ((x * 11 + y * 5) % 256) as u8),
            (16, 16, |_, _| 0),
            (17, 19, |x, y| ((x + y) * 255 / 34) as u8),
            (33, 31, |x, y| ((x * 73 + y * 151 + x * y * 17) % 256) as u8),
        ];
        let mut methods = [false; 2];
        for (width, height, alpha) in cases {
            let rgba: Vec<u8> = (0..height)
                .flat_map(|y| {
                    (0..width).flat_map(move |x| {
                        [
                            (x * 37) as u8,
                            (y * 53) as u8,
                            ((x + y) * 29) as u8,
                            alpha(x, y),
                        ]
                    })
                })
                .collect();
            let expected: Vec<u8> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[3])
                .collect();
            for quality in [0, 50, 75, 100] {
                let webp = encode_rgba(&rgba, width, height, quality).unwrap();
                let scalar = crate::encoder::encode_with_simd(
                    fearless_simd::Fallback::new(),
                    &rgba,
                    width,
                    height,
                    quality,
                    TrellisMode::Restricted,
                )
                .unwrap();
                assert_eq!(webp, scalar, "{width}x{height} q{quality}");
                let payload = alpha_payload(&webp).expect("translucent image carries alpha");
                let method = payload[0] & 3;
                assert!(method <= 1, "{width}x{height}: alpha method {method}");
                methods[usize::from(method)] = true;
                let decoded = image::load_from_memory(&webp)
                    .unwrap_or_else(|error| panic!("{width}x{height} q{quality}: {error}"))
                    .to_rgba8();
                assert_eq!(decoded.dimensions(), (width, height));
                let actual: Vec<u8> = decoded.pixels().map(|pixel| pixel[3]).collect();
                assert_eq!(
                    actual, expected,
                    "{width}x{height} q{quality} method {method}"
                );
            }
        }
        assert_eq!(
            methods,
            [true, true],
            "raw and VP8L alpha must be exercised"
        );
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
