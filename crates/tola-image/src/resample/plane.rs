//! The source sample rows one recipe resamples, and the coverage-weighted form it filters.

use image::metadata::Orientation;

use crate::cancellation::Cancellation;
use crate::recipe::Crop;
use crate::rows::for_each_row;
use crate::{HOLD_IN_MEMORY, pixel_bytes, rgba_buffer};
use crate::{ImageError, ImageResult};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum AlphaRepresentation {
    Opaque,
    Straight,
}

/// RGBA8 rows whose alpha representation travels with every crop and filtering handoff.
///
/// A crop of untransformed pixels stays a window over the decoded buffer. A mirrored or
/// transposed axis is materialized once.
pub(crate) enum SourcePlane<'a> {
    Window {
        pixels: &'a [u8],
        /// Distance between rows, in pixels.
        stride: u32,
        /// First pixel of the first row.
        origin: usize,
        width: u32,
        height: u32,
        alpha: AlphaRepresentation,
    },
    Materialized {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        alpha: AlphaRepresentation,
    },
}

impl SourcePlane<'_> {
    pub(super) fn alpha(&self) -> AlphaRepresentation {
        match self {
            Self::Window { alpha, .. } | Self::Materialized { alpha, .. } => *alpha,
        }
    }

    pub(super) fn mark_opaque_if_covered(
        &mut self,
        cancellation: &dyn Cancellation,
    ) -> ImageResult<()> {
        if self.alpha() == AlphaRepresentation::Opaque {
            return Ok(());
        }
        for y in 0..self.height() {
            cancellation.ensure_active()?;
            if self
                .row(y)
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 255)
            {
                return Ok(());
            }
        }
        match self {
            Self::Window { alpha, .. } | Self::Materialized { alpha, .. } => {
                *alpha = AlphaRepresentation::Opaque;
            }
        }
        Ok(())
    }

    pub(super) fn width(&self) -> u32 {
        match self {
            Self::Window { width, .. } | Self::Materialized { width, .. } => *width,
        }
    }

    pub(super) fn height(&self) -> u32 {
        match self {
            Self::Window { height, .. } | Self::Materialized { height, .. } => *height,
        }
    }

    /// One row as `width * 4` bytes.
    pub(super) fn row(&self, y: u32) -> &[u8] {
        let (pixels, start, width): (&[u8], usize, u32) = match self {
            Self::Window {
                pixels,
                stride,
                origin,
                width,
                ..
            } => (
                *pixels,
                (origin + y as usize * *stride as usize) * 4,
                *width,
            ),
            Self::Materialized { pixels, width, .. } => {
                (pixels.as_slice(), y as usize * *width as usize * 4, *width)
            }
        };
        let bytes = width as usize * 4;
        &pixels[start..start + bytes]
    }
}

/// The crop of the decoded pixels, with EXIF orientation already applied.
pub(super) fn source_plane<'a>(
    rgba: &'a [u8],
    width: u32,
    height: u32,
    orientation: Orientation,
    crop: Crop,
    alpha: AlphaRepresentation,
    cancellation: &dyn Cancellation,
) -> ImageResult<SourcePlane<'a>> {
    if orientation == Orientation::NoTransforms {
        return Ok(SourcePlane::Window {
            pixels: rgba,
            stride: width,
            origin: crop.y as usize * width as usize + crop.x as usize,
            width: crop.width,
            height: crop.height,
            alpha,
        });
    }
    let mut pixels = rgba_buffer(crop.width, crop.height)?;
    let row_bytes = crop.width as usize * 4;
    for_each_row(
        &mut pixels,
        row_bytes,
        || (),
        |(), y, row| {
            cancellation.ensure_active()?;
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (x, y) = source_coordinate(
                    x as u32 + crop.x,
                    y as u32 + crop.y,
                    width,
                    height,
                    orientation,
                );
                let offset = (y as usize * width as usize + x as usize) * 4;
                pixel.copy_from_slice(&rgba[offset..offset + 4]);
            }
            Ok(())
        },
    )?;
    Ok(SourcePlane::Materialized {
        pixels,
        width: crop.width,
        height: crop.height,
        alpha,
    })
}

/// Maps display coordinates back to encoded coordinates, fusing orientation with crop/sampling.
fn source_coordinate(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    orientation: Orientation,
) -> (u32, u32) {
    match orientation {
        Orientation::NoTransforms => (x, y),
        Orientation::FlipHorizontal => (width - 1 - x, y),
        Orientation::Rotate180 => (width - 1 - x, height - 1 - y),
        Orientation::FlipVertical => (x, height - 1 - y),
        Orientation::Rotate90FlipH => (y, x),
        Orientation::Rotate90 => (y, height - 1 - x),
        Orientation::Rotate270FlipH => (width - 1 - y, height - 1 - x),
        Orientation::Rotate270 => (width - 1 - y, x),
    }
}

/// Exact coverage products; dividing by 255 here would lose low-alpha colour.
#[inline]
pub(super) fn premultiply(source: &[u8]) -> [u16; 4] {
    let alpha = u16::from(source[3]);
    [
        u16::from(source[0]) * alpha,
        u16::from(source[1]) * alpha,
        u16::from(source[2]) * alpha,
        255 * alpha,
    ]
}

pub(super) fn premultiply_row(source: &[u8], destination: &mut [[u16; 4]]) {
    for (sample, pixel) in destination.iter_mut().zip(source.as_chunks::<4>().0) {
        *sample = premultiply(pixel);
    }
}

/// A straight crop or the exact coverage products a box stage averaged.
pub(super) enum CoveredPlane<'a> {
    Straight(&'a SourcePlane<'a>),
    Products {
        pixels: Vec<[u16; 4]>,
        width: u32,
        height: u32,
    },
}

impl CoveredPlane<'_> {
    pub(super) fn width(&self) -> u32 {
        match self {
            Self::Straight(plane) => plane.width(),
            Self::Products { width, .. } => *width,
        }
    }

    pub(super) fn height(&self) -> u32 {
        match self {
            Self::Straight(plane) => plane.height(),
            Self::Products { height, .. } => *height,
        }
    }

    pub(super) fn row(&self, y: u32) -> Option<&[[u16; 4]]> {
        match self {
            Self::Straight(_) => None,
            Self::Products { pixels, width, .. } => {
                let start = y as usize * *width as usize;
                Some(&pixels[start..start + *width as usize])
            }
        }
    }
}

/// Premultiplied rows in a ring as consecutive output tap windows move forward.
pub(super) struct PreparedRows {
    samples: Vec<[u16; 4]>,
    width: u32,
    slots: u32,
    first_slot: usize,
    prepared_until: usize,
}

impl PreparedRows {
    pub(super) fn new(width: u32, slots: u32) -> Self {
        Self {
            samples: Vec::new(),
            width,
            slots,
            first_slot: 0,
            prepared_until: 0,
        }
    }

    pub(super) fn prepare(
        &mut self,
        plane: &SourcePlane<'_>,
        first: usize,
        count: usize,
        cancellation: &dyn Cancellation,
    ) -> ImageResult<()> {
        if self.samples.is_empty() {
            let sample_bytes = size_of::<[u16; 4]>();
            let len = pixel_bytes(self.width, self.slots, sample_bytes)? / sample_bytes;
            self.samples
                .try_reserve_exact(len)
                .map_err(|error| ImageError::codec(HOLD_IN_MEMORY, error))?;
            self.samples.resize(len, [0; 4]);
        }
        let slots = self.slots as usize;
        let width = self.width as usize;
        let end = first + count;
        self.first_slot = first % slots;
        // Starts and ends never move backwards within a task. The ring holds at least one
        // complete tap window, so writing the new suffix cannot overwrite a reused row.
        for y in first.max(self.prepared_until)..end {
            cancellation.ensure_active()?;
            let start = y % slots * width;
            premultiply_row(plane.row(y as u32), &mut self.samples[start..][..width]);
        }
        self.prepared_until = end;
        Ok(())
    }

    pub(super) fn row(&self, offset: usize) -> &[[u16; 4]] {
        let slots = self.slots as usize;
        let slot = self.first_slot + offset;
        let slot = if slot >= slots { slot - slots } else { slot };
        let width = self.width as usize;
        &self.samples[slot * width..][..width]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_reads_the_decoded_stride() {
        let decoded: Vec<u8> = (0..64).collect();
        let window = SourcePlane::Window {
            pixels: &decoded,
            stride: 8,
            origin: 2,
            width: 3,
            height: 2,
            alpha: AlphaRepresentation::Straight,
        };
        assert_eq!((window.width(), window.height()), (3, 2));
        assert_eq!(window.row(0), &decoded[8..20]);
        assert_eq!(window.row(1), &decoded[40..52]);
    }
}
