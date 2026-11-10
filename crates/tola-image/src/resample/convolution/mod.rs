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
    AlphaRepresentation, CoveredPlane, PreparedRows, SourcePlane, premultiply_row,
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
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let block = ReducedBlock::of(
        (plane.width(), plane.height()),
        (pixels.width(), pixels.height()),
    );
    if let Some(block) = block {
        if plane.alpha() == AlphaRepresentation::Opaque {
            let reduced = box_averaged(plane, block, cancellation)?;
            convolve_opaque(&reduced, pixels, cancellation)
        } else {
            let reduced = covered_box_averaged(plane, block, cancellation)?;
            convolve_covered(&reduced, pixels, cancellation)
        }
    } else {
        convolved_reduced(plane, pixels, cancellation)
    }
}

struct AxisPasses {
    horizontal: AxisWeights,
    vertical: AxisWeights,
    horizontal_first: bool,
}

impl AxisPasses {
    fn new(
        width: u32,
        height: u32,
        pixels: PixelRecipe,
        cancellation: &dyn Cancellation,
    ) -> ImageResult<Self> {
        Ok(Self {
            horizontal: AxisWeights::new(width, pixels.width(), pixels.filter, cancellation)?,
            vertical: AxisWeights::new(height, pixels.height(), pixels.filter, cancellation)?,
            horizontal_first: horizontal_first(width, height, pixels),
        })
    }
}

pub(super) fn convolved_reduced(
    plane: &SourcePlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    if plane.alpha() == AlphaRepresentation::Opaque {
        convolve_opaque(plane, pixels, cancellation)
    } else {
        convolve_covered(&CoveredPlane::Straight(plane), pixels, cancellation)
    }
}

fn horizontal_first(width: u32, height: u32, pixels: PixelRecipe) -> bool {
    u64::from(pixels.width()) * u64::from(height) <= u64::from(width) * u64::from(pixels.height())
}

/// Target size of a band before adding the tap halo. A band writes at least one output row.
const BAND_SOURCE_ROWS: u32 = 64;
const BAND_BYTES: usize = 2 * 1024 * 1024;

fn band_height(width: u32, input_height: u32, output_height: u32, sample_bytes: usize) -> u32 {
    let row_bytes = work_bytes(width, 1, sample_bytes);
    let source_rows = (BAND_BYTES / row_bytes).min(BAND_SOURCE_ROWS as usize) as u32;
    (source_rows / input_height.div_ceil(output_height)).clamp(1, output_height)
}

fn work_bytes(width: u32, height: u32, sample_bytes: usize) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(sample_bytes)
}

/// Intermediate samples and prepared alpha rows kept by the active convolution workers.
pub(super) fn scratch_bytes(width: u32, height: u32, pixels: PixelRecipe, alpha: bool) -> u64 {
    let sample_bytes = if alpha { 16 } else { 8 };
    let output_work = work_bytes(pixels.width(), pixels.height(), 4);
    let step = u64::from(height.div_ceil(pixels.height()));
    let taps = u64::from(pixels.filter.tap_count_bound(height, pixels.height()));
    let bytes = |width, rows, bytes| (u64::from(width) * u64::from(rows)).saturating_mul(bytes);
    if horizontal_first(width, height, pixels) {
        let band = band_height(pixels.width(), height, pixels.height(), sample_bytes);
        let tasks = pixels.height().div_ceil(band) as usize;
        let work = work_bytes(pixels.width(), height, sample_bytes);
        let workers = row_workers(work.max(output_work)).min(tasks);
        let rows = ((u64::from(band) - 1) * step + taps + 2 * (WINDOW_ROWS as u64 - 1))
            .min(u64::from(height)) as u32;
        let prepared = if alpha {
            bytes(width, WINDOW_ROWS as u32, 8)
        } else {
            0
        };
        bytes(pixels.width(), rows, sample_bytes as u64)
            .saturating_add(prepared)
            .saturating_mul(workers as u64)
    } else {
        let work = work_bytes(width, pixels.height(), sample_bytes);
        let workers = row_workers(work.max(output_work)).min(pixels.height() as usize);
        let prepared = if alpha {
            bytes(width, taps.min(u64::from(height)) as u32, 8)
        } else {
            0
        };
        bytes(width, 1, sample_bytes as u64)
            .saturating_add(prepared)
            .saturating_mul(workers as u64)
    }
}

fn resize_samples<T: Copy + Default>(
    samples: &mut Vec<T>,
    width: u32,
    height: u32,
) -> ImageResult<()> {
    let bytes = size_of::<T>();
    let len = pixel_bytes(width, height, bytes)? / bytes;
    if len > samples.len() {
        samples
            .try_reserve_exact(len - samples.len())
            .map_err(|error| ImageError::codec(HOLD_IN_MEMORY, error))?;
    }
    samples.resize(len, T::default());
    Ok(())
}

fn convolve_opaque(
    plane: &SourcePlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let passes = AxisPasses::new(plane.width(), plane.height(), pixels, cancellation)?;
    let level = Level::new();
    let mut output = rgba_buffer(pixels.width(), pixels.height())?;
    if passes.horizontal_first {
        horizontal_bands(
            &mut output,
            pixels,
            plane.height(),
            &passes.vertical,
            cancellation,
            |_: &mut (), first, rows| {
                horizontal_opaque(
                    plane,
                    &passes.horizontal,
                    first,
                    rows,
                    pixels.width() as usize,
                    level,
                );
                Ok(())
            },
            |taps, weights, row| write_opaque_vertical(taps, weights, row, level),
        )?;
    } else {
        let work = work_bytes(plane.width(), pixels.height(), size_of::<[i16; 4]>());
        for_each_row_with_work(
            &mut output,
            pixels.width() as usize * 4,
            work,
            Vec::new,
            |samples, y, row| {
                cancellation.ensure_active()?;
                if samples.is_empty() {
                    resize_samples(samples, plane.width(), 1)?;
                }
                let (first, weights) = passes.vertical.at(y as u32);
                let tap = |offset| plane.row(first + offset as u32);
                for (index, destination) in samples.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
                    let mut totals = [[0i32; 4]; ACCUMULATE_PIXELS];
                    accumulate_tile(
                        &mut totals[..destination.len()],
                        tap,
                        weights,
                        index * ACCUMULATE_PIXELS,
                        level,
                    );
                    for (destination, total) in destination.iter_mut().zip(totals) {
                        *destination = finish_tile(total);
                    }
                }
                write_opaque_horizontal(samples, &passes.horizontal, row);
                Ok(())
            },
        )?;
    }
    Ok(output)
}

fn convolve_covered(
    plane: &CoveredPlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let passes = AxisPasses::new(plane.width(), plane.height(), pixels, cancellation)?;
    let level = Level::new();
    let (first, second) = if passes.horizontal_first {
        (&passes.horizontal, &passes.vertical)
    } else {
        (&passes.vertical, &passes.horizontal)
    };
    check_coverage_bounds(first, second)?;
    let mut output = rgba_buffer(pixels.width(), pixels.height())?;
    if passes.horizontal_first {
        horizontal_bands(
            &mut output,
            pixels,
            plane.height(),
            &passes.vertical,
            cancellation,
            |prepared, first, rows| {
                horizontal_covered(
                    plane,
                    &passes.horizontal,
                    first,
                    rows,
                    pixels.width() as usize,
                    prepared,
                    level,
                )
            },
            |taps, weights, row| write_covered_vertical(taps, weights, row, level),
        )?;
    } else {
        let work = work_bytes(plane.width(), pixels.height(), size_of::<[i32; 4]>());
        for_each_row_with_work(
            &mut output,
            pixels.width() as usize * 4,
            work,
            || {
                (
                    PreparedRows::new(plane.width(), passes.vertical.max_taps()),
                    Vec::new(),
                )
            },
            |(prepared, samples), y, row| {
                cancellation.ensure_active()?;
                if samples.is_empty() {
                    resize_samples(samples, plane.width(), 1)?;
                }
                let (first, weights) = passes.vertical.at(y as u32);
                if let CoveredPlane::Straight(source) = plane {
                    prepared.prepare(source, first as usize, weights.len(), cancellation)?;
                }
                let tap = |offset| {
                    plane
                        .row(first + offset as u32)
                        .unwrap_or_else(|| prepared.row(offset))
                };
                write_covered_vertical_row(samples, tap, weights, level);
                write_covered_horizontal(samples, &passes.horizontal, row, level);
                Ok(())
            },
        )?;
    }
    Ok(output)
}

/// Each task walks consecutive output bands, reusing the horizontal samples that still lie in
/// the next band's vertical tap window. Four-row alignment preserves the first pass's SIMD groups.
fn horizontal_bands<T: Copy + Default + Send, S: Default + Send>(
    output: &mut [u8],
    pixels: PixelRecipe,
    input_height: u32,
    vertical: &AxisWeights,
    cancellation: &dyn Cancellation,
    filter: impl Fn(&mut S, u32, &mut [T]) -> ImageResult<()> + Sync + Send,
    write: impl Fn(&[T], &[i16], &mut [u8]) + Sync + Send,
) -> ImageResult<()> {
    let band = band_height(
        pixels.width(),
        input_height,
        pixels.height(),
        size_of::<T>(),
    ) as usize;
    let width = pixels.width() as usize;
    let stride = width * 4;
    let work = work_bytes(pixels.width(), input_height, size_of::<T>());
    for_each_row_with_work(
        output,
        band * stride,
        work,
        || (0usize, Vec::<T>::new(), S::default()),
        |(previous, samples, state), index, rows| {
            let y = index * band;
            let (first, _) = vertical.at(y as u32);
            let (last, weights) = vertical.at((y + rows.len() / stride - 1) as u32);
            let first = first as usize / WINDOW_ROWS * WINDOW_ROWS;
            let end = (last as usize + weights.len())
                .next_multiple_of(WINDOW_ROWS)
                .min(input_height as usize);

            let offset = (first - *previous) * width;
            // Both ends of the tap window advance with the output rows.
            let overlap = samples.len().saturating_sub(offset);
            if offset != 0 && overlap != 0 {
                samples.copy_within(offset..offset + overlap, 0);
            }
            samples.truncate(overlap);
            resize_samples(samples, pixels.width(), (end - first) as u32)?;
            *previous = first;
            for (group, destination) in samples[overlap..]
                .chunks_mut(width * WINDOW_ROWS)
                .enumerate()
            {
                cancellation.ensure_active()?;
                let source = first + overlap / width + group * WINDOW_ROWS;
                filter(state, source as u32, destination)?;
            }
            for (offset, row) in rows.chunks_exact_mut(stride).enumerate() {
                cancellation.ensure_active()?;
                let (start, weights) = vertical.at((y + offset) as u32);
                let taps = &samples[(start as usize - first) * width..][..weights.len() * width];
                write(taps, weights, row);
            }
            Ok(())
        },
    )
}

fn horizontal_opaque(
    plane: &SourcePlane<'_>,
    horizontal: &AxisWeights,
    first: u32,
    rows: &mut [[i16; 4]],
    width: usize,
    level: Level,
) {
    if rows.len() == width * WINDOW_ROWS {
        let sources: [&[u8]; WINDOW_ROWS] =
            std::array::from_fn(|row| plane.row(first + row as u32));
        for x in 0..width {
            let (start, weights) = horizontal.at(x as u32);
            let windows = sources.map(|source| &source[start as usize * 4..][..weights.len() * 4]);
            for (row, sample) in dot_window_four(windows, weights, level)
                .into_iter()
                .enumerate()
            {
                rows[row * width + x] = sample;
            }
        }
    } else {
        for (offset, row) in rows.chunks_exact_mut(width).enumerate() {
            let source = plane.row(first + offset as u32);
            for (x, sample) in row.iter_mut().enumerate() {
                let (start, weights) = horizontal.at(x as u32);
                *sample = dot_window(
                    &source[start as usize * 4..][..weights.len() * 4],
                    weights,
                    level,
                );
            }
        }
    }
}

fn horizontal_covered(
    plane: &CoveredPlane<'_>,
    horizontal: &AxisWeights,
    first: u32,
    rows: &mut [[i32; 4]],
    width: usize,
    prepared: &mut Vec<[u16; 4]>,
    level: Level,
) -> ImageResult<()> {
    let count = rows.len() / width;
    let source_width = plane.width() as usize;
    if let CoveredPlane::Straight(source) = plane {
        resize_samples(prepared, plane.width(), count as u32)?;
        for (offset, row) in prepared.chunks_exact_mut(source_width).enumerate() {
            premultiply_row(source.row(first + offset as u32), row);
        }
    }
    let sources: [&[[u16; 4]]; WINDOW_ROWS] = std::array::from_fn(|offset| {
        if offset >= count {
            &[]
        } else {
            plane
                .row(first + offset as u32)
                .unwrap_or_else(|| &prepared[offset * source_width..][..source_width])
        }
    });
    write_covered_horizontal_rows(sources, horizontal, rows, width, level);
    Ok(())
}
