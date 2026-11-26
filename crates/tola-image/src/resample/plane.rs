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

    pub(super) fn mark_opaque_if_covered(&mut self, cancel: &dyn Cancellation) -> ImageResult<()> {
        if self.alpha() == AlphaRepresentation::Opaque {
            return Ok(());
        }
        for y in 0..self.height() {
            cancel.ensure_active()?;
            if self.row(y).as_chunks::<4>().0.iter().any(|p| p[3] != 255) {
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
        let (px, start, w): (&[u8], usize, u32) = match self {
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
        let bytes = w as usize * 4;
        &px[start..start + bytes]
    }
}

/// The crop of the decoded pixels, with EXIF orientation already applied.
pub(super) fn source_plane<'a>(
    rgba: &'a [u8],
    w: u32,
    h: u32,
    orientation: Orientation,
    crop: Crop,
    alpha: AlphaRepresentation,
    cancel: &dyn Cancellation,
) -> ImageResult<SourcePlane<'a>> {
    if orientation == Orientation::NoTransforms {
        return Ok(SourcePlane::Window {
            pixels: rgba,
            stride: w,
            origin: crop.y as usize * w as usize + crop.x as usize,
            width: crop.width,
            height: crop.height,
            alpha,
        });
    }
    let mut px = rgba_buffer(crop.width, crop.height)?;
    let row_bytes = crop.width as usize * 4;
    for_each_row(
        &mut px,
        row_bytes,
        || (),
        |(), y, row| {
            cancel.ensure_active()?;
            for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (sx, sy) =
                    source_coordinate(x as u32 + crop.x, y as u32 + crop.y, w, h, orientation);
                let off = (sy as usize * w as usize + sx as usize) * 4;
                p.copy_from_slice(&rgba[off..off + 4]);
            }
            Ok(())
        },
    )?;
    Ok(SourcePlane::Materialized {
        pixels: px,
        width: crop.width,
        height: crop.height,
        alpha,
    })
}

/// Maps display coordinates back to encoded coordinates, fusing orientation with crop/sampling.
fn source_coordinate(x: u32, y: u32, w: u32, h: u32, orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::NoTransforms => (x, y),
        Orientation::FlipHorizontal => (w - 1 - x, y),
        Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
        Orientation::FlipVertical => (x, h - 1 - y),
        Orientation::Rotate90FlipH => (y, x),
        Orientation::Rotate90 => (y, h - 1 - x),
        Orientation::Rotate270FlipH => (w - 1 - y, h - 1 - x),
        Orientation::Rotate270 => (w - 1 - y, x),
    }
}

/// Exact coverage products; dividing by 255 here would lose low-alpha colour.
#[inline]
pub(super) fn premultiply(src: &[u8]) -> [u16; 4] {
    let alpha = u16::from(src[3]);
    [
        u16::from(src[0]) * alpha,
        u16::from(src[1]) * alpha,
        u16::from(src[2]) * alpha,
        255 * alpha,
    ]
}

pub(super) fn premultiply_row(src: &[u8], dst: &mut [[u16; 4]]) {
    for (sample, p) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
        *sample = premultiply(p);
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
pub(super) struct SampleRows {
    samples: Vec<[u16; 4]>,
    width: u32,
    slots: u32,
    first_slot: usize,
    prepared_until: usize,
}

impl SampleRows {
    pub(super) fn new(w: u32, slots: u32) -> Self {
        Self {
            samples: Vec::new(),
            width: w,
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
        cancel: &dyn Cancellation,
    ) -> ImageResult<()> {
        if self.samples.is_empty() {
            let sample_bytes = size_of::<[u16; 4]>();
            let len = pixel_bytes(self.width, self.slots, sample_bytes)? / sample_bytes;
            self.samples
                .try_reserve_exact(len)
                .map_err(|err| ImageError::codec(HOLD_IN_MEMORY, err))?;
            self.samples.resize(len, [0; 4]);
        }
        let slots = self.slots as usize;
        let w = self.width as usize;
        let end = first + count;
        self.first_slot = first % slots;
        // Starts and ends never move backwards within a task; ring holds >=1 complete tap window.
        for y in first.max(self.prepared_until)..end {
            cancel.ensure_active()?;
            let start = y % slots * w;
            premultiply_row(plane.row(y as u32), &mut self.samples[start..][..w]);
        }
        self.prepared_until = end;
        Ok(())
    }

    pub(super) fn row(&self, off: usize) -> &[[u16; 4]] {
        let slots = self.slots as usize;
        let slot = self.first_slot + off;
        let slot = if slot >= slots { slot - slots } else { slot };
        let w = self.width as usize;
        &self.samples[slot * w..][..w]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_stride_row() {
        let decoded: Vec<u8> = (0..64).collect();
        let win = SourcePlane::Window {
            pixels: &decoded,
            stride: 8,
            origin: 2,
            width: 3,
            height: 2,
            alpha: AlphaRepresentation::Straight,
        };
        assert_eq!((win.width(), win.height()), (3, 2));
        assert_eq!(win.row(0), &decoded[8..20]);
        assert_eq!(win.row(1), &decoded[40..52]);
    }
}
