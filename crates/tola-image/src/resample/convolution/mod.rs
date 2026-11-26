//! The two-pass separable convolution over quantized weights.

use fearless_simd::Level;

use self::intermediate::{
    ACCUMULATE_PIXELS, WINDOW_ROWS, accumulate_tile, dot_window, dot_window_four, finish_tile,
    write_covered_horizontal_rows, write_covered_vertical_row,
};
use self::output::{
    write_covered_horizontal, write_covered_vertical, write_opaque_horizontal,
    write_opaque_vertical,
};
use crate::cancellation::Cancellation;
use crate::recipe::PixelRecipe;
use crate::resample::plane::{
    AlphaRepresentation, CoveredPlane, SampleRows, SourcePlane, premultiply_row,
};
use crate::resample::reduce::{ReducedBlock, box_averaged, covered_box_averaged};
use crate::resample::weights::{AxisWeights, check_coverage_bounds};
use crate::rgba_buffer;
use crate::rows::{for_each_row_with_work, row_workers};
use crate::{HOLD_IN_MEMORY, ImageError, ImageResult, pixel_bytes};

mod intermediate;
mod output;

pub(super) fn convolved_pixels(
    plane: &SourcePlane<'_>,
    px: PixelRecipe,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let block = ReducedBlock::of((plane.width(), plane.height()), (px.width(), px.height()));
    if let Some(block) = block {
        if plane.alpha() == AlphaRepresentation::Opaque {
            let reduced = box_averaged(plane, block, cancel)?;
            convolve_opaque(&reduced, px, cancel)
        } else {
            let reduced = covered_box_averaged(plane, block, cancel)?;
            convolve_covered(&reduced, px, cancel)
        }
    } else {
        convolved_reduced(plane, px, cancel)
    }
}

struct AxisPasses {
    horiz: AxisWeights,
    vert: AxisWeights,
    horiz_first: bool,
}

impl AxisPasses {
    fn new(w: u32, h: u32, px: PixelRecipe, cancel: &dyn Cancellation) -> ImageResult<Self> {
        Ok(Self {
            horiz: AxisWeights::new(w, px.width(), px.filter, cancel)?,
            vert: AxisWeights::new(h, px.height(), px.filter, cancel)?,
            horiz_first: horizontal_first(w, h, px),
        })
    }
}

pub(super) fn convolved_reduced(
    plane: &SourcePlane<'_>,
    px: PixelRecipe,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    if plane.alpha() == AlphaRepresentation::Opaque {
        convolve_opaque(plane, px, cancel)
    } else {
        convolve_covered(&CoveredPlane::Straight(plane), px, cancel)
    }
}

fn horizontal_first(w: u32, h: u32, px: PixelRecipe) -> bool {
    u64::from(px.width()) * u64::from(h) <= u64::from(w) * u64::from(px.height())
}

/// Target size of a band before adding the tap halo. A band writes at least one output row.
const BAND_SOURCE_ROWS: u32 = 64;
const BAND_BYTES: usize = 2 * 1024 * 1024;

fn band_height(w: u32, src_h: u32, dst_h: u32, sample_bytes: usize) -> u32 {
    let row_bytes = work_bytes(w, 1, sample_bytes);
    let src_rows = (BAND_BYTES / row_bytes).min(BAND_SOURCE_ROWS as usize) as u32;
    (src_rows / src_h.div_ceil(dst_h)).clamp(1, dst_h)
}

fn work_bytes(w: u32, h: u32, sample_bytes: usize) -> usize {
    (w as usize)
        .saturating_mul(h as usize)
        .saturating_mul(sample_bytes)
}

/// Intermediate samples and prepared alpha rows kept by the active convolution workers.
pub(super) fn scratch_bytes(w: u32, h: u32, px: PixelRecipe, alpha: bool) -> u64 {
    let sample_bytes = if alpha { 16 } else { 8 };
    let out_work = work_bytes(px.width(), px.height(), 4);
    let step = u64::from(h.div_ceil(px.height()));
    let taps = u64::from(px.filter.tap_count_bound(h, px.height()));
    let bytes = |w, rows, b| (u64::from(w) * u64::from(rows)).saturating_mul(b);
    if horizontal_first(w, h, px) {
        let band = band_height(px.width(), h, px.height(), sample_bytes);
        let tasks = px.height().div_ceil(band) as usize;
        let work = work_bytes(px.width(), h, sample_bytes);
        let workers = row_workers(work.max(out_work)).min(tasks);
        let rows = ((u64::from(band) - 1) * step + taps + 2 * (WINDOW_ROWS as u64 - 1))
            .min(u64::from(h)) as u32;
        let prep = if alpha {
            bytes(w, WINDOW_ROWS as u32, 8)
        } else {
            0
        };
        bytes(px.width(), rows, sample_bytes as u64)
            .saturating_add(prep)
            .saturating_mul(workers as u64)
    } else {
        let work = work_bytes(w, px.height(), sample_bytes);
        let workers = row_workers(work.max(out_work)).min(px.height() as usize);
        let prep = if alpha {
            bytes(w, taps.min(u64::from(h)) as u32, 8)
        } else {
            0
        };
        bytes(w, 1, sample_bytes as u64)
            .saturating_add(prep)
            .saturating_mul(workers as u64)
    }
}

fn resize_samples<T: Copy + Default>(samples: &mut Vec<T>, w: u32, h: u32) -> ImageResult<()> {
    let bytes = size_of::<T>();
    let len = pixel_bytes(w, h, bytes)? / bytes;
    if len > samples.len() {
        samples
            .try_reserve_exact(len - samples.len())
            .map_err(|err| ImageError::codec(HOLD_IN_MEMORY, err))?;
    }
    samples.resize(len, T::default());
    Ok(())
}

fn convolve_opaque(
    plane: &SourcePlane<'_>,
    px: PixelRecipe,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let passes = AxisPasses::new(plane.width(), plane.height(), px, cancel)?;
    let level = Level::new();
    let mut out = rgba_buffer(px.width(), px.height())?;
    if passes.horiz_first {
        horizontal_bands(
            &mut out,
            px,
            plane.height(),
            &passes.vert,
            cancel,
            |_: &mut (), first, rows| {
                horizontal_opaque(
                    plane,
                    &passes.horiz,
                    first,
                    rows,
                    px.width() as usize,
                    level,
                );
                Ok(())
            },
            |taps, weights, row| write_opaque_vertical(taps, weights, row, level),
        )?;
    } else {
        let work = work_bytes(plane.width(), px.height(), size_of::<[i16; 4]>());
        for_each_row_with_work(
            &mut out,
            px.width() as usize * 4,
            work,
            Vec::new,
            |samples, y, row| {
                cancel.ensure_active()?;
                if samples.is_empty() {
                    resize_samples(samples, plane.width(), 1)?;
                }
                let (first, weights) = passes.vert.at(y as u32);
                let tap = |off| plane.row(first + off as u32);
                for (idx, dst) in samples.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
                    let mut totals = [[0i32; 4]; ACCUMULATE_PIXELS];
                    accumulate_tile(
                        &mut totals[..dst.len()],
                        tap,
                        weights,
                        idx * ACCUMULATE_PIXELS,
                        level,
                    );
                    for (dst, total) in dst.iter_mut().zip(totals) {
                        *dst = finish_tile(total);
                    }
                }
                write_opaque_horizontal(samples, &passes.horiz, row);
                Ok(())
            },
        )?;
    }
    Ok(out)
}

fn convolve_covered(
    plane: &CoveredPlane<'_>,
    px: PixelRecipe,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let passes = AxisPasses::new(plane.width(), plane.height(), px, cancel)?;
    let level = Level::new();
    let (first, second) = if passes.horiz_first {
        (&passes.horiz, &passes.vert)
    } else {
        (&passes.vert, &passes.horiz)
    };
    check_coverage_bounds(first, second)?;
    let mut out = rgba_buffer(px.width(), px.height())?;
    if passes.horiz_first {
        horizontal_bands(
            &mut out,
            px,
            plane.height(),
            &passes.vert,
            cancel,
            |prep, first, rows| {
                horizontal_covered(
                    plane,
                    &passes.horiz,
                    first,
                    rows,
                    px.width() as usize,
                    prep,
                    level,
                )
            },
            |taps, weights, row| write_covered_vertical(taps, weights, row, level),
        )?;
    } else {
        let work = work_bytes(plane.width(), px.height(), size_of::<[i32; 4]>());
        for_each_row_with_work(
            &mut out,
            px.width() as usize * 4,
            work,
            || {
                (
                    SampleRows::new(plane.width(), passes.vert.max_taps()),
                    Vec::new(),
                )
            },
            |(prep, samples), y, row| {
                cancel.ensure_active()?;
                if samples.is_empty() {
                    resize_samples(samples, plane.width(), 1)?;
                }
                let (first, weights) = passes.vert.at(y as u32);
                if let CoveredPlane::Straight(src) = plane {
                    prep.prepare(src, first as usize, weights.len(), cancel)?;
                }
                let tap = |off| {
                    plane
                        .row(first + off as u32)
                        .unwrap_or_else(|| prep.row(off))
                };
                write_covered_vertical_row(samples, tap, weights, level);
                write_covered_horizontal(samples, &passes.horiz, row, level);
                Ok(())
            },
        )?;
    }
    Ok(out)
}

/// Walk consecutive output bands, reusing horizontal samples still inside the next vertical window.
/// Four-row alignment preserves the first pass's SIMD groups.
fn horizontal_bands<T: Copy + Default + Send, S: Default + Send>(
    out: &mut [u8],
    px: PixelRecipe,
    src_h: u32,
    vert: &AxisWeights,
    cancel: &dyn Cancellation,
    filter: impl Fn(&mut S, u32, &mut [T]) -> ImageResult<()> + Sync + Send,
    write: impl Fn(&[T], &[i16], &mut [u8]) + Sync + Send,
) -> ImageResult<()> {
    let band = band_height(px.width(), src_h, px.height(), size_of::<T>()) as usize;
    let w = px.width() as usize;
    let stride = w * 4;
    let work = work_bytes(px.width(), src_h, size_of::<T>());
    for_each_row_with_work(
        out,
        band * stride,
        work,
        || (0usize, Vec::<T>::new(), S::default()),
        |(prev, samples, state), idx, rows| {
            let y = idx * band;
            let (first, _) = vert.at(y as u32);
            let (last, weights) = vert.at((y + rows.len() / stride - 1) as u32);
            let first = first as usize / WINDOW_ROWS * WINDOW_ROWS;
            let end = (last as usize + weights.len())
                .next_multiple_of(WINDOW_ROWS)
                .min(src_h as usize);

            let off = (first - *prev) * w;
            let overlap = samples.len().saturating_sub(off);
            if off != 0 && overlap != 0 {
                samples.copy_within(off..off + overlap, 0);
            }
            samples.truncate(overlap);
            resize_samples(samples, px.width(), (end - first) as u32)?;
            *prev = first;
            for (group, dst) in samples[overlap..].chunks_mut(w * WINDOW_ROWS).enumerate() {
                cancel.ensure_active()?;
                let src_y = first + overlap / w + group * WINDOW_ROWS;
                filter(state, src_y as u32, dst)?;
            }
            for (off, row) in rows.chunks_exact_mut(stride).enumerate() {
                cancel.ensure_active()?;
                let (start, weights) = vert.at((y + off) as u32);
                let taps = &samples[(start as usize - first) * w..][..weights.len() * w];
                write(taps, weights, row);
            }
            Ok(())
        },
    )
}

fn horizontal_opaque(
    plane: &SourcePlane<'_>,
    horiz: &AxisWeights,
    first: u32,
    rows: &mut [[i16; 4]],
    w: usize,
    level: Level,
) {
    if rows.len() == w * WINDOW_ROWS {
        let srcs: [&[u8]; WINDOW_ROWS] = std::array::from_fn(|r| plane.row(first + r as u32));
        for x in 0..w {
            let (start, weights) = horiz.at(x as u32);
            let windows = srcs.map(|src| &src[start as usize * 4..][..weights.len() * 4]);
            for (r, sample) in dot_window_four(windows, weights, level)
                .into_iter()
                .enumerate()
            {
                rows[r * w + x] = sample;
            }
        }
    } else {
        for (off, row) in rows.chunks_exact_mut(w).enumerate() {
            let src = plane.row(first + off as u32);
            for (x, sample) in row.iter_mut().enumerate() {
                let (start, weights) = horiz.at(x as u32);
                *sample = dot_window(
                    &src[start as usize * 4..][..weights.len() * 4],
                    weights,
                    level,
                );
            }
        }
    }
}

fn horizontal_covered(
    plane: &CoveredPlane<'_>,
    horiz: &AxisWeights,
    first: u32,
    rows: &mut [[i32; 4]],
    w: usize,
    prep: &mut Vec<[u16; 4]>,
    level: Level,
) -> ImageResult<()> {
    let count = rows.len() / w;
    let src_w = plane.width() as usize;
    if let CoveredPlane::Straight(src) = plane {
        resize_samples(prep, plane.width(), count as u32)?;
        for (off, row) in prep.chunks_exact_mut(src_w).enumerate() {
            premultiply_row(src.row(first + off as u32), row);
        }
    }
    let srcs: [&[[u16; 4]]; WINDOW_ROWS] = std::array::from_fn(|off| {
        if off >= count {
            &[]
        } else {
            plane
                .row(first + off as u32)
                .unwrap_or_else(|| &prep[off * src_w..][..src_w])
        }
    });
    write_covered_horizontal_rows(srcs, horiz, rows, w, level);
    Ok(())
}
