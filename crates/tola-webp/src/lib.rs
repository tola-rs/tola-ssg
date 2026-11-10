//! A pure-Rust, deterministic still-image WebP encoder for `tola-image`.
//!
//! The encoder writes lossy VP8 still images with intra mode search, trellis quantization,
//! integer SIMD transforms, and VP8L-compressed alpha. Within one encoder revision, the same
//! pixels and quality produce the same bytes with scalar or SIMD code, including wasm.
//!
//! The public API is experimental. `tola-image` selects it by default and delegates whole-image
//! lossless WebP to `image-webp`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub use encoder::Error;

use std::io::{self, Write};

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
    encode_rgba_with_mode(rgba, width, height, quality, TrellisMode::Restricted)
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
    let mut options = EncodeOptions::new(quality);
    options.trellis = mode;
    encoder::encode(rgba, width, height, options, &|| false).map(container::WebP::into_bytes)
}

/// Settings for encoding straight-alpha RGBA8 as lossy WebP.
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    /// Detail retained, from 0 to 100.
    pub quality: u8,
    /// Coefficient search used by the encoder.
    pub trellis: TrellisMode,
    /// Maximum concurrent workers, including the calling thread. Zero is treated as one.
    pub workers: usize,
    /// Whether any input pixel has alpha below 255. `None` asks the encoder to check.
    pub has_transparency: Option<bool>,
}

impl EncodeOptions {
    /// Use restricted trellis and the host's available workers.
    pub fn new(quality: u8) -> Self {
        Self::with_workers(
            quality,
            std::thread::available_parallelism().map_or(1, |count| count.get()),
        )
    }

    /// Use restricted trellis and an explicit worker limit.
    pub fn with_workers(quality: u8, workers: usize) -> Self {
        Self {
            quality,
            trellis: TrellisMode::Restricted,
            workers,
            has_transparency: None,
        }
    }
}

/// An encoding or destination write failure.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The image could not be encoded.
    #[error(transparent)]
    Encode(#[from] Error),
    /// The encoded image could not be written.
    #[error(transparent)]
    Write(#[from] io::Error),
}

/// Encode one lossy WebP still image and write it to `destination`.
///
/// The encoder polls `cancelled` while preparing the image. A write error can leave a partial
/// image in the destination.
pub fn encode_rgba_to(
    rgba: &[u8],
    width: u32,
    height: u32,
    options: EncodeOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
    mut destination: impl Write,
) -> Result<(), WriteError> {
    let image = encoder::encode(rgba, width, height, options, cancelled)?;
    encoder::check_cancelled(cancelled)?;
    image.write_to(&mut destination)?;
    Ok(())
}

/// Estimated temporary bytes for restricted encoding of a valid frame, excluding its RGBA input.
/// Encoded partition sizes depend on image content.
pub fn estimated_scratch_bytes(
    width: u32,
    height: u32,
    workers: usize,
    has_transparency: bool,
) -> u64 {
    encoder::estimated_scratch_bytes(width, height, workers, has_transparency)
}

mod alpha;
mod bool_writer;
mod color;
mod container;
mod cost;
mod encoder;
mod macroblock;
mod partitions;
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
    use super::*;

    /// The same image encodes to the same bytes, so a rebuild never churns an unchanged asset.
    #[test]
    fn encoding_is_deterministic() {
        let (width, height) = (257, 513);
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[x as u8, y as u8, (x ^ y) as u8, (x + y) as u8]);
            }
        }
        let encode = |workers| {
            let mut bytes = Vec::new();
            let options = EncodeOptions::with_workers(75, workers);
            encode_rgba_to(&rgba, width, height, options, &|| false, &mut bytes).unwrap();
            bytes
        };
        let serial = encode(1);
        assert_eq!(serial, encode(3));
        assert_eq!(serial, encode(5));
    }

    #[test]
    fn cancellation_stops_a_parallel_encode_before_writing() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (width, height) = (257, 257);
        let rgba = [20, 80, 190, 255].repeat((width * height) as usize);
        let mut options = EncodeOptions::with_workers(75, 2);
        options.has_transparency = Some(false);
        let checks = AtomicUsize::new(0);
        let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 30;
        let mut bytes = Vec::new();
        let result = encode_rgba_to(&rgba, width, height, options, &cancelled, &mut bytes);
        assert!(matches!(result, Err(WriteError::Encode(Error::Cancelled))));
        assert!(bytes.is_empty());
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
