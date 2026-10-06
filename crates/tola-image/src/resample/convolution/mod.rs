//! The two-pass separable convolution over quantized weights.
use std::cell::RefCell;

use anyhow::{Context, Result};
use fearless_simd::Level;

use self::intermediate::{ACCUMULATE_PIXELS, WINDOW_ROWS, accumulate_tile, finish_tile};
use self::intermediate::{accumulate_covered_tile, covered_windows, dot_window, dot_window_four};
use self::output::{LANES, block, covered_taps, taps, write_covered_pixel, write_pixel};
use crate::cancellation::Cancellation;
use crate::pixel_bytes;
use crate::recipe::PixelRecipe;
use crate::resample::plane::{AlphaRepresentation, CoveredPlane, PreparedRows, SourcePlane};
use crate::resample::reduce::{ReducedBlock, box_averaged, covered_box_averaged};
use crate::resample::weights::{AxisWeights, check_coverage_bounds, quantize_coverage};
use crate::rgba_buffer;
use crate::rows::for_each_row;

mod intermediate;
mod output;

enum ConvolutionScratch {
    Opaque(Vec<[i16; 4]>),
    Covered(Vec<[i32; 4]>),
}

thread_local! {
    // One allocation per thread: changing sample width releases the other representation.
    static INTERMEDIATE: RefCell<Option<ConvolutionScratch>> = const { RefCell::new(None) };
}

const RETAINED_INTERMEDIATE_BYTES: usize = 1 << 21;

pub(super) fn convolved_pixels(
    plane: &SourcePlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
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
    ) -> Result<Self> {
        Ok(Self {
            horizontal: AxisWeights::new(width, pixels.width(), pixels.filter, cancellation)?,
            vertical: AxisWeights::new(height, pixels.height(), pixels.filter, cancellation)?,
            horizontal_first: u64::from(pixels.width()) * u64::from(height)
                <= u64::from(width) * u64::from(pixels.height()),
        })
    }
}

pub(super) fn convolved_reduced(
    plane: &SourcePlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    if plane.alpha() == AlphaRepresentation::Opaque {
        convolve_opaque(plane, pixels, cancellation)
    } else {
        convolve_covered(&CoveredPlane::Straight(plane), pixels, cancellation)
    }
}

fn convolve_opaque(
    plane: &SourcePlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    let output_width = pixels.width();
    let output_height = pixels.height();
    let passes = AxisPasses::new(plane.width(), plane.height(), pixels, cancellation)?;
    let (intermediate_width, intermediate_height) = if passes.horizontal_first {
        (output_width, plane.height())
    } else {
        (plane.width(), output_height)
    };
    let samples = pixel_bytes(intermediate_width, intermediate_height, 8)? / 8;
    let mut intermediate = match INTERMEDIATE.with(RefCell::take) {
        Some(ConvolutionScratch::Opaque(pixels)) => pixels,
        _ => Vec::new(),
    };
    intermediate.clear();
    intermediate
        .try_reserve_exact(samples)
        .context("the image is too large to hold in memory; use a smaller image")?;
    intermediate.resize(samples, [0i16; 4]);
    let level = Level::new();
    into_intermediate(
        plane,
        &passes,
        &mut intermediate,
        intermediate_width as usize,
        level,
        cancellation,
    )?;
    cancellation.ensure_active()?;
    let mut output = rgba_buffer(output_width, output_height)?;
    into_output(
        &intermediate,
        intermediate_width as usize,
        &passes,
        &mut output,
        output_width,
        level,
        cancellation,
    )?;
    if intermediate.capacity() <= RETAINED_INTERMEDIATE_BYTES / 8 {
        INTERMEDIATE
            .with(|cell| *cell.borrow_mut() = Some(ConvolutionScratch::Opaque(intermediate)));
    }
    Ok(output)
}

fn convolve_covered(
    plane: &CoveredPlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    let passes = AxisPasses::new(plane.width(), plane.height(), pixels, cancellation)?;
    let (first, second, width, height) = if passes.horizontal_first {
        (
            &passes.horizontal,
            &passes.vertical,
            pixels.width(),
            plane.height(),
        )
    } else {
        (
            &passes.vertical,
            &passes.horizontal,
            plane.width(),
            pixels.height(),
        )
    };
    check_coverage_bounds(first, second)?;
    let samples = pixel_bytes(width, height, 16)? / 16;
    let mut intermediate = match INTERMEDIATE.with(RefCell::take) {
        Some(ConvolutionScratch::Covered(pixels)) => pixels,
        _ => Vec::new(),
    };
    intermediate.clear();
    intermediate
        .try_reserve_exact(samples)
        .context("the image is too large to hold in memory; use a smaller image")?;
    intermediate.resize(samples, [0i32; 4]);
    let level = Level::new();
    into_covered_intermediate(
        plane,
        &passes,
        &mut intermediate,
        width as usize,
        level,
        cancellation,
    )?;
    cancellation.ensure_active()?;
    let mut output = rgba_buffer(pixels.width(), pixels.height())?;
    into_covered_output(
        &intermediate,
        width as usize,
        &passes,
        &mut output,
        pixels.width(),
        level,
        cancellation,
    )?;
    if intermediate.capacity() <= RETAINED_INTERMEDIATE_BYTES / 16 {
        INTERMEDIATE
            .with(|cell| *cell.borrow_mut() = Some(ConvolutionScratch::Covered(intermediate)));
    }
    Ok(output)
}

fn into_intermediate(
    plane: &SourcePlane<'_>,
    passes: &AxisPasses,
    intermediate: &mut [[i16; 4]],
    intermediate_width: usize,
    level: Level,
    cancellation: &dyn Cancellation,
) -> Result<()> {
    if passes.horizontal_first {
        return for_each_row(
            intermediate,
            intermediate_width * WINDOW_ROWS,
            || (),
            |(), block, rows| {
                cancellation.ensure_active()?;
                let first = block * WINDOW_ROWS;
                let row_count = rows.len() / intermediate_width;
                let sources: [&[u8]; WINDOW_ROWS] = std::array::from_fn(|offset| {
                    if offset < row_count {
                        plane.row((first + offset) as u32)
                    } else {
                        &[]
                    }
                });
                if row_count == WINDOW_ROWS {
                    for x in 0..intermediate_width {
                        let (start, weights) = passes.horizontal.at(x as u32);
                        let windows: [&[u8]; WINDOW_ROWS] = std::array::from_fn(|row| {
                            &sources[row][start as usize * 4..][..weights.len() * 4]
                        });
                        for (row, output) in dot_window_four(windows, weights, level)
                            .into_iter()
                            .enumerate()
                        {
                            rows[row * intermediate_width + x] = output;
                        }
                    }
                    return Ok(());
                }
                for (row, source) in sources.iter().enumerate().take(row_count) {
                    for (x, destination) in rows[row * intermediate_width..][..intermediate_width]
                        .iter_mut()
                        .enumerate()
                    {
                        let (start, weights) = passes.horizontal.at(x as u32);
                        *destination = dot_window(
                            &source[start as usize * 4..][..weights.len() * 4],
                            weights,
                            level,
                        );
                    }
                }
                Ok(())
            },
        );
    }
    for_each_row(
        intermediate,
        intermediate_width,
        || (),
        |(), y, row| {
            cancellation.ensure_active()?;
            let (first, weights) = passes.vertical.at(y as u32);
            let tap = |offset| plane.row(first + offset as u32);
            for (index, destination) in row.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
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
            Ok(())
        },
    )
}

fn into_covered_intermediate(
    plane: &CoveredPlane<'_>,
    passes: &AxisPasses,
    intermediate: &mut [[i32; 4]],
    width: usize,
    level: Level,
    cancellation: &dyn Cancellation,
) -> Result<()> {
    if passes.horizontal_first {
        return for_each_row(
            intermediate,
            width * WINDOW_ROWS,
            Vec::<[u16; 4]>::new,
            |prepared, block, rows| {
                cancellation.ensure_active()?;
                let first = block * WINDOW_ROWS;
                let count = rows.len() / width;
                if matches!(plane, CoveredPlane::Straight(_)) {
                    let samples = pixel_bytes(plane.width(), count as u32, 8)? / 8;
                    if prepared.len() < samples {
                        prepared
                            .try_reserve_exact(samples - prepared.len())
                            .context(
                                "the image is too large to hold in memory; use a smaller image",
                            )?;
                        prepared.resize(samples, [0; 4]);
                    }
                    for offset in 0..count {
                        let start = offset * plane.width() as usize;
                        plane.prepare_row(
                            (first + offset) as u32,
                            &mut prepared[start..start + plane.width() as usize],
                        );
                    }
                }
                let sources: [&[[u16; 4]]; WINDOW_ROWS] = std::array::from_fn(|offset| {
                    if offset >= count {
                        &[]
                    } else if let Some(row) = plane.row((first + offset) as u32) {
                        row
                    } else {
                        let start = offset * plane.width() as usize;
                        &prepared[start..start + plane.width() as usize]
                    }
                });
                for x in 0..width {
                    let (start, weights) = passes.horizontal.at(x as u32);
                    if count == WINDOW_ROWS {
                        let windows = std::array::from_fn(|row| {
                            &sources[row][start as usize..start as usize + weights.len()]
                        });
                        for (row, sample) in covered_windows::<WINDOW_ROWS>(windows, weights, level)
                            .into_iter()
                            .enumerate()
                        {
                            rows[row * width + x] = sample;
                        }
                    } else {
                        for (row, source) in sources.iter().take(count).enumerate() {
                            rows[row * width + x] = covered_windows(
                                [&source[start as usize..start as usize + weights.len()]],
                                weights,
                                level,
                            )[0];
                        }
                    }
                }
                Ok(())
            },
        );
    }
    for_each_row(
        intermediate,
        width,
        || PreparedRows::new(plane.width()),
        |prepared, y, row| {
            cancellation.ensure_active()?;
            let (first, weights) = passes.vertical.at(y as u32);
            if matches!(plane, CoveredPlane::Straight(_)) {
                prepared.prepare(plane, first as usize, weights.len(), cancellation)?;
            }
            let tap = |offset| {
                let source = first as usize + offset;
                plane
                    .row(source as u32)
                    .unwrap_or_else(|| prepared.row(source))
            };
            for (index, destination) in row.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
                let mut totals = [[0i64; 4]; ACCUMULATE_PIXELS];
                accumulate_covered_tile(
                    &mut totals[..destination.len()],
                    tap,
                    weights,
                    index * ACCUMULATE_PIXELS,
                    level,
                );
                for (destination, total) in destination.iter_mut().zip(totals) {
                    *destination = total.map(|sum| quantize_coverage(sum) as i32);
                }
            }
            Ok(())
        },
    )
}

fn into_output(
    intermediate: &[[i16; 4]],
    intermediate_width: usize,
    passes: &AxisPasses,
    output: &mut [u8],
    output_width: u32,
    level: Level,
    cancellation: &dyn Cancellation,
) -> Result<()> {
    let samples = intermediate.as_flattened();
    let stride = intermediate_width * 4;
    let width = output_width as usize;
    for_each_row(
        output,
        width * 4,
        || (),
        |(), y, row| {
            cancellation.ensure_active()?;
            if passes.horizontal_first {
                let (start, weights) = passes.vertical.at(y as u32);
                let base = start as usize * stride;
                let mut x = 0;
                while x + LANES <= width {
                    block(
                        level,
                        &samples[base + x * 4..],
                        weights,
                        stride,
                        &mut row[x * 4..][..LANES * 4],
                    );
                    x += LANES;
                }
                while x < width {
                    let totals = taps(
                        |offset| intermediate[(start as usize + offset) * intermediate_width + x],
                        weights,
                    );
                    write_pixel(&mut row[x * 4..][..4], totals);
                    x += 1;
                }
                return Ok(());
            }
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (start, weights) = passes.horizontal.at(x as u32);
                let base = y * intermediate_width + start as usize;
                write_pixel(pixel, taps(|offset| intermediate[base + offset], weights));
            }
            Ok(())
        },
    )
}

fn into_covered_output(
    intermediate: &[[i32; 4]],
    width: usize,
    passes: &AxisPasses,
    output: &mut [u8],
    output_width: u32,
    level: Level,
    cancellation: &dyn Cancellation,
) -> Result<()> {
    for_each_row(
        output,
        output_width as usize * 4,
        || (),
        |(), y, row| {
            cancellation.ensure_active()?;
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let values = if passes.horizontal_first {
                    let (start, weights) = passes.vertical.at(y as u32);
                    covered_taps(
                        |offset| intermediate[(start as usize + offset) * width + x],
                        weights,
                        level,
                    )
                } else {
                    let (start, weights) = passes.horizontal.at(x as u32);
                    let base = y * width + start as usize;
                    covered_taps(|offset| intermediate[base + offset], weights, level)
                };
                write_covered_pixel(pixel, values);
            }
            Ok(())
        },
    )
}
