//! Deterministic image processing: bytes in, encoded variant bytes out.
//!
//! Rows of one image are independent, so resampling and colour conversion divide them across the
//! calling thread's pool once an image is large enough to be worth the hand-off. The variant bytes
//! do not depend on how many threads ran them, and a caller that runs one thread gets the same
//! result without the split.
//!
//! Publication, derivative identity, and request observation belong to the build engine. The
//! pipeline revision in [`IMAGE_PIPELINE_REVISION`] is part of the identity of every variant this
//! crate produces.
//!
//! # Memory and concurrency
//!
//! A decoded source and each resampled result own an RGBA8 plane. Convolution holds an
//! intermediate plane with eight bytes per sample for fully covered crops, sixteen when coverage
//! varies. Covered box reduction and prepared rows use eight-byte color–alpha products.
//! Color conversion and encoding may need their own buffers; rows share plane ownership.
//! Hosts must bound the images and geometries they admit concurrently and the worker count.
//! Retained convolution scratch is capped at 2 MiB per thread. Source decoding reserves its raw
//! and normalization planes from a 512 MiB budget; codec scratch remains best-effort, and this
//! budget does not cover resampling, encoding, or other images a host keeps alive.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cancellation;
mod codec;
mod recipe;
mod resample;
mod rows;

pub use cancellation::{
    Cancellation, ImageCancelled, NO_CANCELLATION, NoCancellation, ensure_active_if_present,
};
pub use codec::{DecodedSource, inspect, render};
pub use recipe::{
    Background, ImageRecipe, OutputFormat, PixelRecipe, ResizeFilter, ResizeOperation,
    ResizeOptions,
};
pub use resample::ResampledPixels;

/// Revision of the image pipeline: increment it whenever anything that changes the bytes this crate
/// produces changes, whether that is a resampling, color, codec, or metadata policy of our own or
/// a dependency, feature, or native compiler option that carries one. Every derivative name and
/// every request descriptor carries this revision, so a derivative rendered under another revision
/// is never reused as if it were this one.
pub const IMAGE_PIPELINE_REVISION: u32 = 5;

/// The container an image's own bytes are in.
///
/// [`OutputFormat`] is what a request asks a derivative to be encoded as; this is what the source's
/// own bytes already are.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ImageFormat {
    /// Lossy, and it stores no transparency.
    Jpeg,
    /// Lossless, and stores transparency.
    Png,
    /// Lossless unless encoded with a quality, and stores transparency.
    WebP,
    /// Read as a still frame; an animated source is refused.
    Gif,
    /// Read for its embedded colour profile, which its V4 and V5 headers carry.
    Bmp,
    /// Measured but never resized.
    Svg,
}

impl ImageFormat {
    /// The extension a derivative of this format is published with; JPEG publishes as `jpg`.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Svg => "svg",
        }
    }

    /// The media type a derivative of this format is published with.
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Svg => "image/svg+xml",
        }
    }
}

/// What a source reports about itself, read from its container rather than its pixels.
///
/// Both dimensions are display dimensions: an EXIF orientation is already applied, so a rotated
/// photograph reports the size its pixels will be resized at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageMetadata {
    /// Display width.
    pub width: u32,
    /// Display height.
    pub height: u32,
    /// The container these pixels were decoded from.
    pub format: ImageFormat,
    /// Whether the source declares alpha or transparent regions, even if no pixel uses them.
    pub has_alpha: bool,
    /// Whether the container's own encoding discarded information.
    pub is_lossy: bool,
}

fn check_dimensions(width: u32, height: u32) -> anyhow::Result<()> {
    anyhow::ensure!(width > 0 && height > 0, "image dimensions must be positive");
    Ok(())
}

/// Byte length of one decoded plane, rejecting sizes that cannot be addressed.
pub(crate) fn pixel_bytes(
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> anyhow::Result<usize> {
    use anyhow::Context;

    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .filter(|bytes| *bytes <= isize::MAX as usize)
        .context("the image is too large to hold in memory; use a smaller image")
}

pub(crate) fn rgba_buffer(width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;

    let bytes = pixel_bytes(width, height, 4)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(bytes)
        .context("the image is too large to hold in memory; use a smaller image")?;
    rgba.resize(bytes, 0);
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extension names the published derivative and the media type is reported to the site
    /// author, so both are part of what a site exposes. JPEG publishes as `jpg`: changing that
    /// would rename every JPEG derivative, invalidating every reference to one.
    #[test]
    fn formats_name_their_own_published_files() {
        for (format, extension, mime) in [
            (ImageFormat::Jpeg, "jpg", "image/jpeg"),
            (ImageFormat::Png, "png", "image/png"),
            (ImageFormat::WebP, "webp", "image/webp"),
            (ImageFormat::Gif, "gif", "image/gif"),
            (ImageFormat::Bmp, "bmp", "image/bmp"),
            (ImageFormat::Svg, "svg", "image/svg+xml"),
        ] {
            assert_eq!(format.extension(), extension, "{format:?}");
            assert_eq!(format.mime(), mime, "{format:?}");
        }
    }
}
