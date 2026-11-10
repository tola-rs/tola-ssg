//! WebP frame encoding and macroblock traversal.

use std::ops::Range;

use crate::EncodeOptions;
use crate::color::{RgbaFrame, YUV_BYTES_PER_MACROBLOCK, YuvRow};
use crate::container;
use crate::macroblock::{CHROMA_SIDE, CoefficientSpool, Luma, MACROBLOCK_SIDE, Macroblock};
use crate::partitions::encode_frame;
use crate::quantize;
#[cfg(not(target_arch = "wasm32"))]
use crate::reconstruction::Plane;
use crate::reconstruction::Reconstruction;
#[cfg(not(target_arch = "wasm32"))]
use crate::token::Edges;
use crate::token::TokenContexts;
use crate::trellis;
use crate::vp8;
use fearless_simd::{Level, Simd, dispatch};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, SyncSender};

mod search;

use self::search::{Quantizer, prefers_intra4, search_macroblock};

/// Keep cancellation responsive without polling for every block.
const CANCEL_MACROBLOCKS: usize = 8;

/// Below this area, thread setup costs more than the overlapped work.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_MIN_PIXELS: usize = 16_384;
/// Two vertical strips only repay their duplicate rings and handoffs above this area.
#[cfg(not(target_arch = "wasm32"))]
const STRIP_MIN_PIXELS: usize = 256 * 256;
/// Narrow frames have too little work per row to amortise the handoff.
#[cfg(not(target_arch = "wasm32"))]
const STRIP_MIN_COLUMNS: usize = 8;
/// Thresholds for increasing the analysis from two strips to four.
#[cfg(not(target_arch = "wasm32"))]
const FOUR_STRIP_MIN_PIXELS: usize = 128 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const FOUR_STRIP_MIN_COLUMNS: usize = 16;

/// Large allocations made by the restricted encoder, beyond the caller's RGBA buffer.
pub(crate) fn estimated_scratch_bytes(
    width: u32,
    height: u32,
    workers: usize,
    has_transparency: bool,
) -> u64 {
    let pixels = u64::from(width) * u64::from(height);
    let columns = u64::from(width.div_ceil(MACROBLOCK_SIDE as u32));
    let blocks = columns * u64::from(height.div_ceil(MACROBLOCK_SIDE as u32));
    let strips = strip_count(width as usize, height as usize, workers.max(1)) as u64;
    let modes = blocks * size_of::<Macroblock>() as u64;
    let coefficients = CoefficientSpool::estimated_bytes(blocks, strips);
    let yuv = columns * YUV_BYTES_PER_MACROBLOCK as u64;
    let row_rings = u64::from(width) * 48;
    let analysis = yuv + row_rings + trellis::restricted_scratch_bytes(strips) + modes;
    // Partition lengths depend on image content. Allow two RGBA-sized buffers for writes and
    // the temporary comparison of entropy coding choices.
    let partitions = modes.saturating_add(pixels.saturating_mul(8));
    // Mixed alpha can hold the residual plane, VP8L's expansion and frame, and the ALPH chunk.
    let alpha = if has_transparency {
        pixels.saturating_mul(9)
    } else {
        0
    };
    coefficients
        .saturating_add(analysis.max(partitions))
        .saturating_add(alpha)
}

/// Why an input could not be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A dimension is zero or exceeds VP8's 14-bit maximum.
    #[error("the image is empty or larger than WebP's 16,383-pixel limit")]
    Dimensions,
    /// The pixel buffer length does not match `width × height × 4`.
    #[error("the pixel buffer does not match width × height × 4")]
    BufferLength,
    /// The selected prediction modes exceed VP8's 19-bit frame-header length.
    #[error("the image's prediction modes exceed WebP's frame-header limit")]
    HeaderTooLarge,
    /// The caller cancelled the encode.
    #[error("WebP encoding was cancelled")]
    Cancelled,
}

pub(crate) fn check_cancelled(cancelled: &(dyn Fn() -> bool + Sync)) -> Result<(), Error> {
    if cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

pub(crate) fn encode(
    rgba: &[u8],
    width: u32,
    height: u32,
    options: EncodeOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<container::WebP, Error> {
    check_cancelled(cancelled)?;
    const MAX_DIMENSION: u32 = 16_383;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(Error::Dimensions);
    }
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(Error::BufferLength);
    }
    dispatch!(Level::new(), simd => encode_with_simd(simd, rgba, width, height, options, cancelled))
}

fn encode_with_simd<S: Simd>(
    simd: S,
    rgba: &[u8],
    width: u32,
    height: u32,
    options: EncodeOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<container::WebP, Error> {
    let (pixel_width, pixel_height) = (width as usize, height as usize);
    let alpha = || {
        crate::alpha::encode(
            rgba,
            pixel_width,
            pixel_height,
            options.has_transparency,
            cancelled,
        )
    };
    let frame = |workers| {
        encode_frame_from_rgba(
            simd,
            rgba,
            pixel_width,
            pixel_height,
            EncodeOptions { workers, ..options },
            cancelled,
        )
    };
    let workers = options.workers.max(1);
    #[cfg(not(target_arch = "wasm32"))]
    if workers > 1
        && options.has_transparency != Some(false)
        && pixel_width * pixel_height >= PARALLEL_MIN_PIXELS
    {
        return std::thread::scope(|scope| {
            let alpha = scope.spawn(alpha);
            let frame = frame(workers - 1)?;
            let alpha = alpha.join().expect("alpha encoding panicked")?;
            Ok(container::WebP {
                width,
                height,
                frame,
                alpha,
            })
        });
    }
    let frame = frame(workers)?;
    Ok(container::WebP {
        width,
        height,
        frame,
        alpha: alpha()?,
    })
}

fn strip_count(width: usize, height: usize, workers: usize) -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (width, height, workers);
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let columns = width.div_ceil(MACROBLOCK_SIDE);
        let area = width * height;
        if area < STRIP_MIN_PIXELS || columns < STRIP_MIN_COLUMNS || height <= MACROBLOCK_SIDE {
            return 1;
        }
        let wanted = if area >= FOUR_STRIP_MIN_PIXELS && columns >= FOUR_STRIP_MIN_COLUMNS {
            4
        } else {
            2
        };
        wanted.min(workers)
    }
}

fn encode_frame_from_rgba<S: Simd>(
    simd: S,
    rgba: &[u8],
    width: usize,
    height: usize,
    options: EncodeOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<vp8::Frame, Error> {
    let EncodeOptions {
        quality,
        trellis,
        workers,
        ..
    } = options;
    let source = RgbaFrame::new(rgba, width, height);
    let (quantizer, lambda) = quantize::frame_quantizer(quality);
    let search = Quantizer::new(simd, quantize::factors(quantizer), lambda, trellis);
    let filter_level = (quantizer / 2).min(63);
    let intra4_first = prefers_intra4(simd, source);
    #[cfg(not(target_arch = "wasm32"))]
    let strips = strip_count(width, height, workers);
    #[cfg(not(target_arch = "wasm32"))]
    if strips > 1 {
        let strips = analyze_strips(source, search, strips, intra4_first, cancelled)?;
        return encode_frame(
            width,
            height,
            quantizer,
            filter_level,
            &strips,
            cancelled,
            workers,
        );
    }
    let strip = analyze_rows(source, search, intra4_first, cancelled)?;
    encode_frame(
        width,
        height,
        quantizer,
        filter_level,
        &[strip],
        cancelled,
        workers,
    )
}

/// One vertical strip's macroblocks in row order and their local coefficients.
pub(crate) struct MacroblockStrip {
    pub(crate) columns: usize,
    pub(crate) macroblocks: Vec<Macroblock>,
    pub(crate) spool: CoefficientSpool,
}

fn analyze_rows<S: Simd>(
    source: RgbaFrame<'_>,
    search: Quantizer<S>,
    intra4_first: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<MacroblockStrip, Error> {
    let columns = source.columns();
    let rows = source.rows();
    let mut raster = MacroblockRaster::full_frame(source, search, intra4_first);
    let mut macroblocks = Vec::with_capacity(columns * rows);
    for y in 0..rows {
        check_cancelled(cancelled)?;
        raster.load_row(y);
        for x in 0..columns {
            if x != 0 && x % CANCEL_MACROBLOCKS == 0 {
                check_cancelled(cancelled)?;
            }
            macroblocks.push(raster.encode(x, y));
        }
    }
    Ok(MacroblockStrip {
        columns,
        macroblocks,
        spool: raster.spool,
    })
}

/// The rolling reconstruction and coding contexts advanced by each accepted macroblock.
struct MacroblockRaster<'a, S> {
    source: RgbaFrame<'a>,
    row: YuvRow,
    reconstructed: Reconstruction,
    quantizer: Quantizer<S>,
    tokens: TokenContexts,
    modes: vp8::BmodeContexts,
    spool: CoefficientSpool,
    intra4_first: bool,
}

impl<'a, S: Simd> MacroblockRaster<'a, S> {
    fn new(
        source: RgbaFrame<'a>,
        reconstructed: Reconstruction,
        quantizer: Quantizer<S>,
        columns: Range<usize>,
        intra4_first: bool,
    ) -> Self {
        Self {
            source,
            row: YuvRow::new(columns),
            reconstructed,
            quantizer,
            tokens: TokenContexts::new(source.columns()),
            modes: vp8::BmodeContexts::new(source.columns()),
            spool: CoefficientSpool::new(),
            intra4_first,
        }
    }

    fn full_frame(source: RgbaFrame<'a>, quantizer: Quantizer<S>, intra4_first: bool) -> Self {
        let columns = source.columns();
        let reconstructed = Reconstruction::new(
            columns * MACROBLOCK_SIDE,
            columns * CHROMA_SIDE,
            source.rows(),
        );
        Self::new(source, reconstructed, quantizer, 0..columns, intra4_first)
    }

    fn load_row(&mut self, y: usize) {
        self.row.load(self.quantizer.simd, self.source, y);
    }

    fn encode(&mut self, x: usize, y: usize) -> Macroblock {
        let dense = search_macroblock(
            &self.row,
            &mut self.reconstructed,
            &self.quantizer,
            self.tokens.contexts(x),
            &self.modes,
            (x, y),
            self.intra4_first,
        );
        match &dense.luma {
            Luma::Intra16 { mode, .. } => self.modes.advance_intra(x, *mode),
            Luma::Intra4 { modes } => self.modes.advance_b_pred(x, modes),
        }
        let macroblock = Macroblock::pack(dense, &mut self.spool);
        self.tokens.advance(x, &macroblock);
        macroblock
    }
}

/// One row's right-edge samples and contexts for the next strip.
#[cfg(not(target_arch = "wasm32"))]
struct StripBoundary {
    luma: [u8; MACROBLOCK_SIDE],
    chroma: [[u8; CHROMA_SIDE]; 2],
    tokens: Edges,
    modes: [u8; 4],
}

#[cfg(not(target_arch = "wasm32"))]
impl StripBoundary {
    fn read<S>(raster: &MacroblockRaster<'_, S>, x: usize, y: usize) -> Self {
        fn column<const SIDE: usize>(plane: &Plane, x: usize, y: usize) -> [u8; SIDE] {
            std::array::from_fn(|row| plane.sample(x * SIDE - 1, y * SIDE + row))
        }
        Self {
            luma: column(&raster.reconstructed.y, x, y),
            chroma: [&raster.reconstructed.u, &raster.reconstructed.v]
                .map(|plane| column(plane, x, y)),
            tokens: raster.tokens.left(),
            modes: raster.modes.left_modes(),
        }
    }

    fn write<S>(self, raster: &mut MacroblockRaster<'_, S>, x: usize, y: usize) {
        fn column<const SIDE: usize>(plane: &mut Plane, x: usize, y: usize, pixels: [u8; SIDE]) {
            for (row, pixel) in pixels.into_iter().enumerate() {
                plane.write(y * SIDE + row, x * SIDE - 1, &[pixel]);
            }
        }
        column(&mut raster.reconstructed.y, x, y, self.luma);
        let [u, v] = self.chroma;
        column(&mut raster.reconstructed.u, x, y, u);
        column(&mut raster.reconstructed.v, x, y, v);
        raster.tokens.set_left(self.tokens);
        raster.modes.set_left_modes(self.modes);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct StripChannels {
    from_left: Option<Receiver<StripBoundary>>,
    to_right: Option<SyncSender<StripBoundary>>,
    from_above_right: Option<Receiver<[u8; 4]>>,
    to_below_left: Option<SyncSender<[u8; 4]>>,
}

/// Frame inputs shared by every strip.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct StripContext<'a> {
    source: RgbaFrame<'a>,
    intra4_first: bool,
    columns: usize,
    rows: usize,
    cancelled: &'a (dyn Fn() -> bool + Sync),
}

/// The left strip sends the right edge of the current row. The right strip sends
/// above-right samples for the next row. Macroblock coordinates stay frame-relative.
#[cfg(not(target_arch = "wasm32"))]
fn analyze_strips<S: Simd>(
    source: RgbaFrame<'_>,
    quantizer: Quantizer<S>,
    strips: usize,
    intra4_first: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<MacroblockStrip>, Error> {
    let columns = source.columns();
    let rows = source.rows();
    let context = StripContext {
        source,
        intra4_first,
        columns,
        rows,
        cancelled,
    };

    let mut channels: Vec<_> = (0..strips).map(|_| StripChannels::default()).collect();
    for right in 1..strips {
        let left = right - 1;
        let (to_right, from_left) = mpsc::sync_channel(1);
        let (to_below_left, from_above_right) = mpsc::sync_channel(1);
        channels[left].to_right = Some(to_right);
        channels[left].from_above_right = Some(from_above_right);
        channels[right].from_left = Some(from_left);
        channels[right].to_below_left = Some(to_below_left);
    }

    std::thread::scope(|scope| {
        let run = |index, quantizer, channels| {
            let start = index * columns / strips;
            let end = (index + 1) * columns / strips;
            analyze_strip(context, quantizer, start, end, channels)
        };
        let last = channels.pop().expect("there are at least two strips");
        let handles: Vec<_> = channels
            .into_iter()
            .enumerate()
            .map(|(index, channels)| {
                let quantizer = quantizer.fork();
                scope.spawn(move || run(index, quantizer, channels))
            })
            .collect();
        let last = run(strips - 1, quantizer, last);

        handles
            .into_iter()
            .map(|handle| handle.join().expect("an analysis strip panicked"))
            .chain(std::iter::once(last))
            .collect()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn analyze_strip<S: Simd>(
    context: StripContext<'_>,
    quantizer: Quantizer<S>,
    x_start: usize,
    x_end: usize,
    channels: StripChannels,
) -> Result<MacroblockStrip, Error> {
    let StripContext {
        source,
        intra4_first,
        columns,
        rows,
        cancelled,
    } = context;
    let reconstructed = Reconstruction::strip(
        columns * MACROBLOCK_SIDE,
        columns * CHROMA_SIDE,
        rows,
        x_start,
        x_end,
    );
    let mut raster = MacroblockRaster::new(
        source,
        reconstructed,
        quantizer,
        x_start..x_end,
        intra4_first,
    );
    let width = x_end - x_start;
    let mut macroblocks = Vec::with_capacity(width * rows);

    for y in 0..rows {
        check_cancelled(cancelled)?;
        raster.load_row(y);
        if let Some(receiver) = &channels.from_left {
            receiver
                .recv()
                .map_err(|_| Error::Cancelled)?
                .write(&mut raster, x_start, y);
        }
        for x in x_start..x_end {
            if x != x_start && (x - x_start) % CANCEL_MACROBLOCKS == 0 {
                check_cancelled(cancelled)?;
            }
            // The last macroblock of the row is the only one that reads the right strip's
            // above-right edge; waiting here lets the left strip run its whole row first.
            if x + 1 == x_end
                && y > 0
                && let Some(receiver) = &channels.from_above_right
            {
                let pixels = receiver.recv().map_err(|_| Error::Cancelled)?;
                raster.reconstructed.y.write(
                    y * MACROBLOCK_SIDE - 1,
                    x_end * MACROBLOCK_SIDE,
                    &pixels,
                );
            }
            let macroblock = raster.encode(x, y);
            if x == x_start
                && y + 1 < rows
                && let Some(sender) = &channels.to_below_left
            {
                let pixels = raster.reconstructed.y.slice(
                    x_start * MACROBLOCK_SIDE,
                    (y + 1) * MACROBLOCK_SIDE - 1,
                    4,
                );
                sender
                    .send(pixels.try_into().unwrap())
                    .map_err(|_| Error::Cancelled)?;
            }
            macroblocks.push(macroblock);
        }

        if let Some(sender) = &channels.to_right {
            sender
                .send(StripBoundary::read(&raster, x_end, y))
                .map_err(|_| Error::Cancelled)?;
        }
    }

    Ok(MacroblockStrip {
        columns: x_end - x_start,
        macroblocks,
        spool: raster.spool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrellisMode;
    use crate::encode_rgba;

    fn gradient(width: usize, height: usize) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    96,
                    255,
                ]);
            }
        }
        rgba
    }

    #[test]
    fn gradient_retains_its_colors() {
        let rgba = gradient(32, 32);
        let webp = encode_rgba(&rgba, 32, 32, 80).unwrap();
        let decoded = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (32, 32));
        let mut total = 0u64;
        for (source, output) in rgba.as_chunks::<4>().0.iter().zip(decoded.pixels()) {
            for (&value, &channel) in source.iter().take(3).zip(output.0.iter().take(3)) {
                total += u64::from(value.abs_diff(channel));
            }
        }
        let mean = total / (32 * 32 * 3) as u64;
        assert!(mean < 32, "mean absolute error {mean} is too high");
    }

    #[test]
    fn invalid_images_are_rejected() {
        assert_eq!(encode_rgba(&[], 0, 1, 80), Err(Error::Dimensions));
        assert_eq!(encode_rgba(&[], 1, 0, 80), Err(Error::Dimensions));
        assert_eq!(encode_rgba(&[], 16_384, 1, 80), Err(Error::Dimensions));
        assert_eq!(encode_rgba(&[], 1, 16_384, 80), Err(Error::Dimensions));
        assert_eq!(
            encode_rgba(&[], u32::MAX, u32::MAX, 80),
            Err(Error::Dimensions)
        );
        assert_eq!(encode_rgba(&[0; 4], 2, 2, 80), Err(Error::BufferLength));
    }

    #[test]
    fn reconstruction_matches_vp8_decoder() {
        for (width, height) in [(1, 33), (17, 1), (7, 7), (15, 15), (31, 17), (65, 33)] {
            let mut random = 1u32;
            let rgba: Vec<u8> = (0..width * height)
                .flat_map(|_| {
                    let mut pixel = [255; 4];
                    for channel in &mut pixel[..3] {
                        random ^= random << 13;
                        random ^= random >> 17;
                        random ^= random << 5;
                        *channel = (random >> 24) as u8;
                    }
                    pixel
                })
                .collect();
            let simd = fearless_simd::Fallback::new();
            let source = RgbaFrame::new(&rgba, width, height);
            let columns = source.columns();
            let rows = source.rows();
            let intra4_first = prefers_intra4(simd, source);
            for quality in [0, 25, 75, 100] {
                let (quantizer, lambda) = quantize::frame_quantizer(quality);
                let factors = quantize::factors(quantizer);
                let search = Quantizer::new(simd, factors, lambda, TrellisMode::Exact);
                let mut raster = MacroblockRaster::full_frame(source, search, intra4_first);
                let mut macroblocks = Vec::with_capacity(columns * rows);
                let mut expected = [MACROBLOCK_SIDE, CHROMA_SIDE, CHROMA_SIDE]
                    .map(|side| vec![0; columns * rows * side * side]);
                for y in 0..rows {
                    raster.load_row(y);
                    for x in 0..columns {
                        macroblocks.push(raster.encode(x, y));
                        for ((plane, side), pixels) in [
                            (&raster.reconstructed.y, MACROBLOCK_SIDE),
                            (&raster.reconstructed.u, CHROMA_SIDE),
                            (&raster.reconstructed.v, CHROMA_SIDE),
                        ]
                        .into_iter()
                        .zip(&mut expected)
                        {
                            for row in 0..side {
                                let start = (y * side + row) * plane.frame_stride() + x * side;
                                pixels[start..start + side].copy_from_slice(
                                    &plane.row(y * side + row)[x * side..(x + 1) * side],
                                );
                            }
                        }
                    }
                }
                let strip = MacroblockStrip {
                    columns,
                    macroblocks,
                    spool: raster.spool,
                };
                // Disabling the loop filter isolates predictor and coefficient reconstruction.
                let frame =
                    encode_frame(width, height, quantizer, 0, &[strip], &|| false, 1).unwrap();
                let decoded = image_webp::vp8::Vp8Decoder::decode_frame(std::io::Cursor::new(
                    frame.chunks().concat(),
                ))
                .unwrap();
                for ((name, expected), actual) in ["Y", "U", "V"].into_iter().zip(&expected).zip([
                    &decoded.ybuf,
                    &decoded.ubuf,
                    &decoded.vbuf,
                ]) {
                    assert_eq!(expected, actual, "{width}x{height} q{quality} {name}");
                }
            }
        }
    }
}
