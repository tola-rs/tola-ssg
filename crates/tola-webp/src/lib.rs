//! Pure-Rust deterministic lossy WebP encoder.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub use encoder::Error;

use std::io::{self, Write};

/// Coefficient trellis mode for [`encode_rgba_with_mode`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrellisMode {
    /// Exhaustive trellis over the `0..=2047` magnitude range.
    #[default]
    Exact,
    /// Two-candidate restricted trellis.
    Restricted,
}

/// Encode straight-alpha RGBA8 pixels as a lossy WebP image.
pub fn encode_rgba(rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, Error> {
    encode_rgba_with_mode(rgba, width, height, quality, TrellisMode::Restricted)
}

/// Encode straight-alpha RGBA8 pixels using an explicit [`TrellisMode`].
pub fn encode_rgba_with_mode(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    mode: TrellisMode,
) -> Result<Vec<u8>, Error> {
    let mut opts = EncodeOptions::new(quality);
    opts.trellis = mode;
    encoder::encode(rgba, width, height, opts, &|| false).map(container::WebP::into_bytes)
}

/// Options for encoding straight-alpha RGBA8 pixels as lossy WebP.
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    /// Target quality in `0..=100`.
    pub quality: u8,
    /// Coefficient trellis mode.
    pub trellis: TrellisMode,
    /// Maximum concurrent workers, clamped to at least one.
    pub workers: usize,
    /// Known alpha presence, or `None` to inspect the input.
    pub has_transparency: Option<bool>,
}

impl EncodeOptions {
    /// Create options with restricted trellis and host parallelism.
    pub fn new(quality: u8) -> Self {
        Self::with_workers(
            quality,
            std::thread::available_parallelism().map_or(1, |n| n.get()),
        )
    }

    /// Create options with restricted trellis and an explicit worker limit.
    pub fn with_workers(quality: u8, workers: usize) -> Self {
        Self {
            quality,
            trellis: TrellisMode::Restricted,
            workers,
            has_transparency: None,
        }
    }
}

/// Failure while encoding or writing a WebP image.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// Image encoding failed.
    #[error(transparent)]
    Encode(#[from] Error),
    /// Writing to the destination failed.
    #[error(transparent)]
    Write(#[from] io::Error),
}

/// Encode one lossy WebP image and write it to `destination`.
pub fn encode_rgba_to(
    rgba: &[u8],
    width: u32,
    height: u32,
    options: EncodeOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
    mut destination: impl Write,
) -> Result<(), WriteError> {
    let webp = encoder::encode(rgba, width, height, options, cancelled)?;
    encoder::check_cancelled(cancelled)?;
    webp.write_to(&mut destination)?;
    Ok(())
}

/// Estimated scratch bytes for restricted encoding, excluding the RGBA input.
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

    #[test]
    fn deterministic_encode() {
        let (w, h) = (257, 513);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[x as u8, y as u8, (x ^ y) as u8, (x + y) as u8]);
            }
        }
        let encode = |workers| {
            let mut buf = Vec::new();
            let opts = EncodeOptions::with_workers(75, workers);
            encode_rgba_to(&rgba, w, h, opts, &|| false, &mut buf).unwrap();
            buf
        };
        let serial = encode(1);
        assert_eq!(serial, encode(3));
        assert_eq!(serial, encode(5));
    }

    #[test]
    fn parallel_encode_cancel() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (w, h) = (257, 257);
        let rgba = [20, 80, 190, 255].repeat((w * h) as usize);
        let mut opts = EncodeOptions::with_workers(75, 2);
        opts.has_transparency = Some(false);
        let checks = AtomicUsize::new(0);
        let cancel = || checks.fetch_add(1, Ordering::Relaxed) >= 30;
        let mut buf = Vec::new();
        let res = encode_rgba_to(&rgba, w, h, opts, &cancel, &mut buf);
        assert!(matches!(res, Err(WriteError::Encode(Error::Cancelled))));
        assert!(buf.is_empty());
    }

    fn alpha_payload(webp: &[u8]) -> Option<&[u8]> {
        let mut pos = 12;
        while pos < webp.len() {
            let len = u32::from_le_bytes(webp[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let payload = &webp[pos + 8..pos + 8 + len];
            if &webp[pos..pos + 4] == b"ALPH" {
                return Some(payload);
            }
            pos += 8 + len + (len & 1);
        }
        None
    }

    #[test]
    fn opaque_omits_alpha() {
        for (w, h) in [(1, 1), (1, 17), (17, 1), (17, 19)] {
            let rgba = [37, 123, 211, 255].repeat((w * h) as usize);
            let webp = encode_rgba(&rgba, w, h, 75).unwrap();
            assert!(alpha_payload(&webp).is_none(), "{w}x{h}");
            let decoded = image::load_from_memory(&webp).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (w, h));
            assert!(decoded.pixels().all(|px| px[3] == 255));
        }
    }
}
