//! The still-image encoder: intra mode search and residual coding, one macroblock at a time.

use crate::TrellisMode;
use crate::bool_writer::BoolWriter;
use crate::color::{self, YuvPlanes};
use crate::container;
use crate::macroblock::{
    CHROMA_SIDE, CoefficientSpool, DenseMacroblock, Luma, MACROBLOCK_SIDE, Macroblock,
};
use crate::predict::{
    self, B_DC, B_HD, B_HE, B_HU, B_LD, B_RD, B_TM, B_VE, B_VL, B_VR, DC, HORIZONTAL, TRUE_MOTION,
    VERTICAL,
};
use crate::quantize::{self, Factors};
use crate::reconstruction::{Plane, Reconstruction};
use crate::tables::DEFAULT_COEFF_PROBS;
#[cfg(not(target_arch = "wasm32"))]
use crate::token::Edges;
use crate::token::{
    COEFFICIENT_PROBABILITIES, CoefficientPlane, FittedPartition, MacroblockContexts,
    TokenContexts, TokenCosts, TokenCounts, TokenSink, TokenWriter, fit_probs,
};
use crate::transform;
use crate::trellis;
use crate::vp8;
use fearless_simd::{Level, Simd, dispatch};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, SyncSender};

// Each YUV420 chroma sample represents a 2×2 luma cell.
const CHROMA_SAMPLE_AREA: u64 = 4;

/// Below this area, thread setup costs more than the overlapped work.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_MIN_PIXELS: usize = 16_384;
/// Partitions are overlapped only when the image has enough macroblocks.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_MIN_MACROBLOCKS: usize = 64;
/// Two vertical strips only repay their duplicate rings and handoffs above this area.
#[cfg(not(target_arch = "wasm32"))]
const STRIP_MIN_PIXELS: usize = 256 * 256;
/// Narrow frames have too little work per row to amortise the handoff.
#[cfg(not(target_arch = "wasm32"))]
const STRIP_MIN_COLUMNS: usize = 8;
/// Four strips are measured for larger frames; narrower or smaller frames use two.
#[cfg(not(target_arch = "wasm32"))]
const MAX_STRIPS: usize = 4;

/// Why an input could not be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A dimension is zero or exceeds VP8's 14-bit maximum.
    Dimensions,
    /// The pixel buffer length does not match `width × height × 4`.
    BufferLength,
    /// The selected prediction modes exceed VP8's 19-bit frame-header length.
    HeaderTooLarge,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dimensions => {
                formatter.write_str("the image is empty or larger than WebP's 16,383-pixel limit")
            }
            Self::BufferLength => {
                formatter.write_str("the pixel buffer does not match width × height × 4")
            }
            Self::HeaderTooLarge => {
                formatter.write_str("the image's prediction modes exceed WebP's frame-header limit")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Encode straight-alpha RGBA8 pixels as one lossy WebP still image.
pub(crate) fn encode_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, Error> {
    encode_rgba_with_mode(rgba, width, height, quality, TrellisMode::Restricted)
}

pub(crate) fn encode_rgba_with_mode(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    mode: TrellisMode,
) -> Result<Vec<u8>, Error> {
    dispatch!(Level::new(), simd => encode_with_simd(simd, rgba, width, height, quality, mode))
}

pub(crate) fn encode_with_simd<S: Simd>(
    simd: S,
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    mode: TrellisMode,
) -> Result<Vec<u8>, Error> {
    const MAX_DIMENSION: u32 = 16_383;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(Error::Dimensions);
    }
    let width = width as usize;
    let height = height as usize;
    if rgba.len() != width * height * 4 {
        return Err(Error::BufferLength);
    }

    #[cfg(not(target_arch = "wasm32"))]
    let (frame, alpha) = if width * height >= PARALLEL_MIN_PIXELS {
        std::thread::scope(|scope| {
            let alpha = scope.spawn(|| crate::alpha::encode(rgba, width, height));
            let frame = encode_frame_from_rgba(simd, rgba, width, height, quality, mode)?;
            let alpha = alpha.join().expect("alpha encoding panicked");
            Ok::<_, Error>((frame, alpha))
        })?
    } else {
        (
            encode_frame_from_rgba(simd, rgba, width, height, quality, mode)?,
            crate::alpha::encode(rgba, width, height),
        )
    };
    #[cfg(target_arch = "wasm32")]
    let (frame, alpha) = (
        encode_frame_from_rgba(simd, rgba, width, height, quality, mode)?,
        crate::alpha::encode(rgba, width, height),
    );
    Ok(container::riff_webp(
        width as u32,
        height as u32,
        &frame,
        alpha.as_deref(),
    ))
}

fn encode_frame_from_rgba<S: Simd>(
    simd: S,
    rgba: &[u8],
    width: usize,
    height: usize,
    quality: u8,
    mode: TrellisMode,
) -> Result<Vec<u8>, Error> {
    encode_frame_from_rgba_with_strips(
        simd,
        rgba,
        width,
        height,
        quality,
        mode,
        default_strips(width, height),
    )
}

fn default_strips(width: usize, height: usize) -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (width, height);
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let columns = width.div_ceil(MACROBLOCK_SIDE);
        let rows = height.div_ceil(MACROBLOCK_SIDE);
        let area = width * height;
        if area < STRIP_MIN_PIXELS || columns < STRIP_MIN_COLUMNS || rows < 2 {
            return 1;
        }
        let cores = std::thread::available_parallelism().map_or(1, |count| count.get());
        let wanted = if area >= 128 * 1024 && columns >= 16 {
            4
        } else {
            2
        };
        wanted.min(cores).min(MAX_STRIPS).min(columns)
    }
}

fn encode_frame_from_rgba_with_strips<S: Simd>(
    simd: S,
    rgba: &[u8],
    width: usize,
    height: usize,
    quality: u8,
    mode: TrellisMode,
    strips: usize,
) -> Result<Vec<u8>, Error> {
    let source = color::from_rgba(simd, rgba, width, height);
    let quantizer = quantize::quantizer_index(quality);
    let factors = quantize::factors(quantizer);
    let lambda = 16 * u64::from(quantizer) * u64::from(quantizer);
    // The index-7 RD point sits 5% above the nominal lambda at quality 95.
    const Q95_LAMBDA_PERCENT: u64 = 105;
    let lambda = if quality == 95 {
        lambda * Q95_LAMBDA_PERCENT / 100
    } else {
        lambda
    };
    let filter_level = (quantizer / 2).min(63);

    #[cfg(not(target_arch = "wasm32"))]
    if strips > 1 && width.div_ceil(MACROBLOCK_SIDE) > 1 {
        let (macroblocks, spool) = analyze_strips(
            simd,
            &source,
            factors,
            lambda,
            strips,
            i4_first(&source),
            mode,
        );
        drop(source);
        return encode_frame(width, height, quantizer, filter_level, &macroblocks, &spool);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = strips;

    let mut analysis = Macroblocks::new(simd, &source, factors, lambda, mode);
    let macroblocks: Vec<_> = analysis.by_ref().collect();
    let spool = analysis.into_coefficient_spool();
    drop(source);
    encode_frame(width, height, quantizer, filter_level, &macroblocks, &spool)
}

/// Exact acceptance is only worth its cost while the estimated gain is small or the image is
/// tiny; above these bounds the fitted table is emitted unchanged.
const SMALL_FIT_SAVINGS: u64 = 8 * 256;
const SMALL_IMAGE_AREA: usize = 1 << 14;

fn encode_frame(
    width: usize,
    height: usize,
    quantizer: u8,
    filter_level: u8,
    macroblocks: &[Macroblock],
    spool: &CoefficientSpool,
) -> Result<Vec<u8>, Error> {
    let macroblock_columns = width.div_ceil(MACROBLOCK_SIDE);
    let fitted_probs = {
        let mut tokens = TokenWriter::new(macroblock_columns);
        let mut counts = TokenCounts::new();
        for (index, macroblock) in macroblocks.iter().enumerate() {
            tokens.write_macroblock(&mut counts, index % macroblock_columns, macroblock, spool);
        }
        fit_probs(&counts.into_counts())
    };
    let exact = fitted_probs
        .as_ref()
        .is_some_and(|fit| should_check_exactly(fit.savings, width, height));
    let (first_bytes, residual_bytes) = match fitted_probs {
        None => write_partitions(
            macroblock_columns,
            quantizer,
            filter_level,
            &DEFAULT_COEFF_PROBS,
            macroblocks,
            spool,
        ),
        Some(fit) if !exact => write_partitions(
            macroblock_columns,
            quantizer,
            filter_level,
            &fit.probs,
            macroblocks,
            spool,
        ),
        Some(fit) => {
            let mut first_partition = BoolWriter::new();
            vp8::write_frame_header(
                &mut first_partition,
                quantizer,
                filter_level,
                &DEFAULT_COEFF_PROBS,
            );
            // The fitted emission doubles as its replay; the same bools drive a default-table
            // length state, so exact acceptance costs no third token traversal.
            let mut partition = FittedPartition::new(&fit.probs);
            write_residual(
                &mut partition,
                macroblock_columns,
                macroblocks,
                spool,
                fit.probs,
            );
            let (fitted_bytes, default_residual_len) = partition.finish();
            if fitted_bytes.len() < default_residual_len {
                let mut candidate = BoolWriter::new();
                vp8::write_frame_header(&mut candidate, quantizer, filter_level, &fit.probs);
                {
                    let mut tee = ModeTee {
                        default: &mut first_partition,
                        candidate: &mut candidate,
                    };
                    write_modes(&mut tee, macroblock_columns, macroblocks);
                }
                let default_first = first_partition.finish();
                let candidate_first = candidate.finish();
                // Both first partitions and both residual lengths are exact, so the accepted
                // frame cannot exceed the default frame.
                if candidate_first.len() < 1 << 19
                    && candidate_first.len() + fitted_bytes.len()
                        < default_first.len() + default_residual_len
                {
                    (candidate_first, fitted_bytes)
                } else {
                    (
                        default_first,
                        default_residual(macroblock_columns, macroblocks, spool),
                    )
                }
            } else {
                write_modes(&mut first_partition, macroblock_columns, macroblocks);
                (
                    first_partition.finish(),
                    default_residual(macroblock_columns, macroblocks, spool),
                )
            }
        }
    };

    vp8::assemble_frame(width as u16, height as u16, &first_bytes, &residual_bytes)
        .ok_or(Error::HeaderTooLarge)
}

/// Exact acceptance is only worth its cost while the estimated gain is small or the image is
/// tiny; above these bounds the fitted table is emitted unchanged.
fn should_check_exactly(savings: u64, width: usize, height: usize) -> bool {
    savings <= SMALL_FIT_SAVINGS || width * height < SMALL_IMAGE_AREA
}

fn write_partitions(
    macroblock_columns: usize,
    quantizer: u8,
    filter_level: u8,
    probs: &[u8; COEFFICIENT_PROBABILITIES],
    macroblocks: &[Macroblock],
    spool: &CoefficientSpool,
) -> (Vec<u8>, Vec<u8>) {
    #[cfg(not(target_arch = "wasm32"))]
    if macroblocks.len() >= PARALLEL_MIN_MACROBLOCKS {
        return std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                write_mode_partition(
                    macroblock_columns,
                    quantizer,
                    filter_level,
                    probs,
                    macroblocks,
                )
            });
            let residual = write_residual_partition(macroblock_columns, probs, macroblocks, spool);
            (first.join().expect("mode partition panicked"), residual)
        });
    }
    (
        write_mode_partition(
            macroblock_columns,
            quantizer,
            filter_level,
            probs,
            macroblocks,
        ),
        write_residual_partition(macroblock_columns, probs, macroblocks, spool),
    )
}

fn write_mode_partition(
    macroblock_columns: usize,
    quantizer: u8,
    filter_level: u8,
    probs: &[u8; COEFFICIENT_PROBABILITIES],
    macroblocks: &[Macroblock],
) -> Vec<u8> {
    let mut first_partition = BoolWriter::new();
    vp8::write_frame_header(&mut first_partition, quantizer, filter_level, probs);
    let mut modes = vp8::BmodeContexts::new(macroblock_columns);
    for (index, macroblock) in macroblocks.iter().enumerate() {
        let x = index % macroblock_columns;
        match &macroblock.luma {
            Luma::Intra16 { mode, .. } => {
                modes.write_intra(&mut first_partition, x, *mode, macroblock.chroma_mode);
            }
            Luma::Intra4 { modes: subblocks } => {
                modes.write_b_pred(&mut first_partition, x, subblocks, macroblock.chroma_mode);
            }
        }
    }
    first_partition.finish()
}

fn write_residual_partition(
    macroblock_columns: usize,
    probs: &[u8; COEFFICIENT_PROBABILITIES],
    macroblocks: &[Macroblock],
    spool: &CoefficientSpool,
) -> Vec<u8> {
    let mut residual_partition = BoolWriter::new();
    let mut tokens = TokenWriter::with_probs(macroblock_columns, *probs);
    for (index, macroblock) in macroblocks.iter().enumerate() {
        tokens.write_macroblock(
            &mut residual_partition,
            index % macroblock_columns,
            macroblock,
            spool,
        );
    }
    residual_partition.finish()
}

/// Feed one mode sequence to two first partitions whose coefficient headers differ.
struct ModeTee<'a> {
    default: &'a mut BoolWriter,
    candidate: &'a mut BoolWriter,
}

impl TokenSink for ModeTee<'_> {
    fn write_bool(&mut self, probability: u8, value: bool) {
        self.default.write_bool(probability, value);
        self.candidate.write_bool(probability, value);
    }

    fn write_tree_from(&mut self, tree: &[i8], probabilities: &[u8], value: u8, start: usize) {
        self.default
            .write_tree_from(tree, probabilities, value, start);
        self.candidate
            .write_tree_from(tree, probabilities, value, start);
    }
}

fn write_modes<W: TokenSink>(
    writer: &mut W,
    macroblock_columns: usize,
    macroblocks: &[Macroblock],
) {
    let mut modes = vp8::BmodeContexts::new(macroblock_columns);
    for (index, macroblock) in macroblocks.iter().enumerate() {
        let x = index % macroblock_columns;
        match &macroblock.luma {
            Luma::Intra16 { mode, .. } => {
                modes.write_intra(writer, x, *mode, macroblock.chroma_mode);
            }
            Luma::Intra4 { modes: subblocks } => {
                modes.write_b_pred(writer, x, subblocks, macroblock.chroma_mode);
            }
        }
    }
}

fn write_residual<W: TokenSink>(
    writer: &mut W,
    macroblock_columns: usize,
    macroblocks: &[Macroblock],
    spool: &CoefficientSpool,
    probs: [u8; COEFFICIENT_PROBABILITIES],
) {
    let mut tokens = TokenWriter::with_probs(macroblock_columns, probs);
    for (index, macroblock) in macroblocks.iter().enumerate() {
        tokens.write_macroblock(writer, index % macroblock_columns, macroblock, spool);
    }
}

fn default_residual(
    macroblock_columns: usize,
    macroblocks: &[Macroblock],
    spool: &CoefficientSpool,
) -> Vec<u8> {
    let mut partition = BoolWriter::new();
    write_residual(
        &mut partition,
        macroblock_columns,
        macroblocks,
        spool,
        DEFAULT_COEFF_PROBS,
    );
    partition.finish()
}

struct Quantizer<S> {
    simd: S,
    factors: Factors,
    matrices: [quantize::Matrix; 3],
    trellis: trellis::Quantizer,
    costs: TokenCosts,
    lambda: u64,
}

#[derive(Clone, Copy, Debug)]
enum Quantization {
    Nearest,
    Trellis,
}

impl<S: Simd> Quantizer<S> {
    fn new(simd: S, factors: Factors, lambda: u64, mode: TrellisMode) -> Self {
        let costs = TokenCosts::default();
        Self {
            simd,
            factors,
            matrices: [
                quantize::Matrix::new(factors.y_dc, factors.y_ac),
                quantize::Matrix::new(factors.y2_dc, factors.y2_ac),
                quantize::Matrix::new(factors.chroma_dc, factors.chroma_ac),
            ],
            trellis: trellis::Quantizer::new(
                factors,
                lambda / 4,
                lambda / (4 * CHROMA_SAMPLE_AREA),
                &costs,
                mode,
            ),
            costs,
            lambda,
        }
    }

    fn block(
        &self,
        coefficients: &[i32; 16],
        plane: CoefficientPlane,
        context: usize,
        method: Quantization,
    ) -> [i16; 16] {
        let matrix = match plane {
            CoefficientPlane::LumaAc | CoefficientPlane::Luma => &self.matrices[0],
            CoefficientPlane::LumaDc => &self.matrices[1],
            CoefficientPlane::Chroma => &self.matrices[2],
        };
        match method {
            Quantization::Nearest => {
                let mut levels = matrix.quantize(self.simd, coefficients);
                if matches!(plane, CoefficientPlane::LumaAc) {
                    levels[0] = 0;
                }
                levels
            }
            Quantization::Trellis => self.trellis.quantize(coefficients, plane, context),
        }
    }

    fn score(&self, distortion: u64, rate: impl FnOnce() -> u64) -> u64 {
        distortion * 256
            + if self.lambda == 0 {
                0
            } else {
                self.lambda * rate() / 256
            }
    }

    fn chroma_score(
        &self,
        u: &ChromaCandidate,
        v: &ChromaCandidate,
        mut neighbors: MacroblockContexts,
        zero_luma_rate: u64,
        mode: u8,
    ) -> u64 {
        self.score((u.sse + v.sse) * CHROMA_SAMPLE_AREA, || {
            neighbors.chroma_cost(&self.costs, &u.levels, &v.levels)
                + zero_luma_rate
                + vp8::chroma_mode_cost(mode)
        })
    }
}

/// Mean 4x4 luma activity above which the intra4 search is expected to win often enough
/// to run first. Both orders use exact bounds, so this only changes how much work the
/// macroblock search performs; selected modes and output bytes are unaffected.
///
/// Calibrated between the smooth photograph (about 970) and gallery-1 (about 2840), with
/// enough headroom that smooth sources stay on the intra16-first order.
const I4_FIRST_ACTIVITY: u64 = 1200;

/// Macroblock rows and columns sampled when classifying source activity.
const ACTIVITY_GRID: usize = 8;

/// Whether this source is textured enough to search intra4 before intra16.
///
/// The sampler walks an 8x8 macroblock grid and sums the absolute differences of
/// horizontally and vertically adjacent samples inside each 4x4 block. This is an integer
/// content measure, so it is independent of quality and runtime timing.
fn i4_first(source: &YuvPlanes) -> bool {
    let stride = source.y_stride;
    let columns = source.y_stride / MACROBLOCK_SIDE;
    let rows = source.y.len() / source.y_stride / MACROBLOCK_SIDE;
    let step_x = columns.div_ceil(ACTIVITY_GRID).max(1);
    let step_y = rows.div_ceil(ACTIVITY_GRID).max(1);
    let mut activity = 0u64;
    let mut samples = 0u64;
    for macroblock_y in (0..rows).step_by(step_y) {
        for macroblock_x in (0..columns).step_by(step_x) {
            let base = macroblock_y * MACROBLOCK_SIDE * stride + macroblock_x * MACROBLOCK_SIDE;
            for block_y in 0..4 {
                for block_x in 0..4 {
                    for row in 0..4 {
                        let row_start = base + (block_y * 4 + row) * stride + block_x * 4;
                        let mut previous = source.y[row_start];
                        for column in 1..4 {
                            let value = source.y[row_start + column];
                            activity += u64::from(value.abs_diff(previous));
                            previous = value;
                        }
                    }
                    for column in 0..4 {
                        let column_start = base + block_y * 4 * stride + block_x * 4 + column;
                        let mut previous = source.y[column_start];
                        for row in 1..4 {
                            let value = source.y[column_start + row * stride];
                            activity += u64::from(value.abs_diff(previous));
                            previous = value;
                        }
                    }
                }
            }
            samples += 1;
        }
    }
    activity / samples >= I4_FIRST_ACTIVITY
}

struct Macroblocks<'a, S> {
    source: &'a YuvPlanes,
    reconstructed: Reconstruction,
    quantizer: Quantizer<S>,
    tokens: TokenContexts,
    modes: vp8::BmodeContexts,
    spool: CoefficientSpool,
    columns: usize,
    count: usize,
    next_index: usize,
    i4_first: bool,
}

impl<'a, S: Simd> Macroblocks<'a, S> {
    fn new(
        simd: S,
        source: &'a YuvPlanes,
        factors: Factors,
        lambda: u64,
        mode: TrellisMode,
    ) -> Self {
        let columns = source.y_stride / MACROBLOCK_SIDE;
        let rows = source.y.len() / source.y_stride / MACROBLOCK_SIDE;
        Self {
            source,
            reconstructed: Reconstruction::new(source.y_stride, source.chroma_stride, rows),
            quantizer: Quantizer::new(simd, factors, lambda, mode),
            tokens: TokenContexts::new(columns),
            modes: vp8::BmodeContexts::new(columns),
            spool: CoefficientSpool::new(),
            columns,
            count: columns * rows,
            next_index: 0,
            i4_first: i4_first(source),
        }
    }

    fn into_coefficient_spool(self) -> CoefficientSpool {
        self.spool
    }
}

impl<S: Simd> Iterator for Macroblocks<'_, S> {
    type Item = Macroblock;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_index == self.count {
            return None;
        }
        let x = self.next_index % self.columns;
        let y = self.next_index / self.columns;
        let dense = encode_macroblock(
            self.source,
            &mut self.reconstructed,
            &self.quantizer,
            &self.tokens,
            &self.modes,
            (x, y),
            self.i4_first,
        );
        match &dense.luma {
            Luma::Intra16 { mode, .. } => self.modes.advance_intra(x, *mode),
            Luma::Intra4 { modes } => self.modes.advance_b_pred(x, modes),
        }
        let macroblock = Macroblock::pack(dense, &mut self.spool);
        self.tokens.advance(x, &macroblock);
        self.next_index += 1;
        Some(macroblock)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.count - self.next_index;
        (remaining, Some(remaining))
    }
}

impl<S: Simd> ExactSizeIterator for Macroblocks<'_, S> {}

/// One row's right-edge samples and contexts for the next strip.
#[cfg(not(target_arch = "wasm32"))]
struct StripBoundary {
    luma: [u8; 16],
    chroma: [[u8; 8]; 2],
    tokens: Edges,
    modes: [u8; 4],
}

#[cfg(not(target_arch = "wasm32"))]
struct StripResult {
    macroblocks: Vec<Macroblock>,
    spool: CoefficientSpool,
}

/// What every strip shares; only the x range and channels differ per strip.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct StripContext<'a, S: Simd> {
    simd: S,
    source: &'a YuvPlanes,
    factors: Factors,
    lambda: u64,
    mode: TrellisMode,
    i4_first: bool,
    columns: usize,
    rows: usize,
}

/// Split the raster into vertical strips and schedule them with a one-row skew: while strip
/// `k` encodes row `r`, strip `k - 1` encodes `r + 1` and strip `k + 1` encodes `r - 1`.
/// Macroblock coordinates stay global; only storage is strip-local.
#[cfg(not(target_arch = "wasm32"))]
fn analyze_strips<S: Simd>(
    simd: S,
    source: &YuvPlanes,
    factors: Factors,
    lambda: u64,
    strips: usize,
    i4_first: bool,
    mode: TrellisMode,
) -> (Vec<Macroblock>, CoefficientSpool) {
    let columns = source.y_stride / MACROBLOCK_SIDE;
    let rows = source.y.len() / source.y_stride / MACROBLOCK_SIDE;
    let strips = strips.min(columns);
    debug_assert!(strips >= 2);
    let context = StripContext {
        simd,
        source,
        factors,
        lambda,
        mode,
        i4_first,
        columns,
        rows,
    };

    let mut left_txs = Vec::with_capacity(strips - 1);
    let mut left_rxs = Vec::with_capacity(strips - 1);
    let mut above_right_txs = Vec::with_capacity(strips - 1);
    let mut above_right_rxs = Vec::with_capacity(strips - 1);
    for _ in 0..strips - 1 {
        let (left_tx, left_rx) = mpsc::sync_channel(1);
        let (above_right_tx, above_right_rx) = mpsc::sync_channel(1);
        left_txs.push(Some(left_tx));
        left_rxs.push(Some(left_rx));
        above_right_txs.push(Some(above_right_tx));
        above_right_rxs.push(Some(above_right_rx));
    }

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(strips);
        for k in 0..strips {
            let x_start = k * columns / strips;
            let x_end = (k + 1) * columns / strips;
            let left_rx = if k > 0 { left_rxs[k - 1].take() } else { None };
            let left_tx = left_txs.get_mut(k).and_then(Option::take);
            let above_right_rx = above_right_rxs.get_mut(k).and_then(Option::take);
            let above_right_tx = if k > 0 {
                above_right_txs[k - 1].take()
            } else {
                None
            };
            handles.push(scope.spawn(move || {
                analyze_strip(
                    context,
                    x_start,
                    x_end,
                    left_rx,
                    above_right_rx,
                    left_tx,
                    above_right_tx,
                )
            }));
        }
        let mut results = Vec::with_capacity(strips);
        for handle in handles {
            results.push(handle.join().expect("an analysis strip panicked"));
        }

        let mut macroblocks = Vec::with_capacity(columns * rows);
        let mut spool = CoefficientSpool::new();
        let bases: Vec<u32> = results
            .iter_mut()
            .map(|result| {
                let strip = std::mem::replace(&mut result.spool, CoefficientSpool::new());
                spool.absorb(strip)
            })
            .collect();
        for y in 0..rows {
            for (k, result) in results.iter().enumerate() {
                let x_start = k * columns / strips;
                let x_end = (k + 1) * columns / strips;
                let width = x_end - x_start;
                let start = y * width;
                for macroblock in &result.macroblocks[start..start + width] {
                    macroblocks.push(macroblock.rebase(bases[k]));
                }
            }
        }
        (macroblocks, spool)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn analyze_strip<S: Simd>(
    context: StripContext<'_, S>,
    x_start: usize,
    x_end: usize,
    left_rx: Option<Receiver<StripBoundary>>,
    above_right_rx: Option<Receiver<[u8; 4]>>,
    left_tx: Option<SyncSender<StripBoundary>>,
    above_right_tx: Option<SyncSender<[u8; 4]>>,
) -> StripResult {
    let StripContext {
        simd,
        source,
        factors,
        lambda,
        mode,
        i4_first,
        columns,
        rows,
    } = context;
    let mut reconstructed =
        Reconstruction::strip(source.y_stride, source.chroma_stride, rows, x_start, x_end);
    let quantizer = Quantizer::new(simd, factors, lambda, mode);
    let mut tokens = TokenContexts::new(columns);
    let mut modes = vp8::BmodeContexts::new(columns);
    let mut spool = CoefficientSpool::new();
    let width = x_end - x_start;
    let mut macroblocks = Vec::with_capacity(width * rows);

    for y in 0..rows {
        if let Some(receiver) = &left_rx {
            let boundary = receiver.recv().expect("the left strip stopped");
            for (row, &value) in boundary.luma.iter().enumerate() {
                reconstructed.y.write(
                    y * MACROBLOCK_SIDE + row,
                    x_start * MACROBLOCK_SIDE - 1,
                    &[value],
                );
            }
            for (plane, values) in [
                (&mut reconstructed.u, &boundary.chroma[0]),
                (&mut reconstructed.v, &boundary.chroma[1]),
            ] {
                for (row, &value) in values.iter().enumerate() {
                    plane.write(y * CHROMA_SIDE + row, x_start * CHROMA_SIDE - 1, &[value]);
                }
            }
            tokens.set_left(boundary.tokens);
            modes.set_left_modes(boundary.modes);
        }
        for x in x_start..x_end {
            // The last macroblock of the row is the only one that reads the right strip's
            // above-right edge; waiting here lets the left strip run its whole row first.
            if x + 1 == x_end
                && y > 0
                && let Some(receiver) = &above_right_rx
            {
                let pixels = receiver.recv().expect("the right strip stopped");
                reconstructed
                    .y
                    .write(y * MACROBLOCK_SIDE - 1, x_end * MACROBLOCK_SIDE, &pixels);
            }
            let dense = encode_macroblock(
                source,
                &mut reconstructed,
                &quantizer,
                &tokens,
                &modes,
                (x, y),
                i4_first,
            );
            match &dense.luma {
                Luma::Intra16 { mode, .. } => modes.advance_intra(x, *mode),
                Luma::Intra4 { modes: subblocks } => modes.advance_b_pred(x, subblocks),
            }
            let macroblock = Macroblock::pack(dense, &mut spool);
            tokens.advance(x, &macroblock);
            if x == x_start
                && y + 1 < rows
                && let Some(sender) = &above_right_tx
            {
                let pixels: [u8; 4] = std::array::from_fn(|column| {
                    reconstructed.y.sample(
                        x_start * 16 + column,
                        y * MACROBLOCK_SIDE + (MACROBLOCK_SIDE - 1),
                    )
                });
                sender
                    .send(pixels)
                    .expect("the left strip stopped listening");
            }
            macroblocks.push(macroblock);
        }

        if let Some(sender) = &left_tx {
            let boundary = StripBoundary {
                luma: std::array::from_fn(|row| {
                    reconstructed
                        .y
                        .sample(x_end * MACROBLOCK_SIDE - 1, y * MACROBLOCK_SIDE + row)
                }),
                chroma: [
                    std::array::from_fn(|row| {
                        reconstructed
                            .u
                            .sample(x_end * CHROMA_SIDE - 1, y * CHROMA_SIDE + row)
                    }),
                    std::array::from_fn(|row| {
                        reconstructed
                            .v
                            .sample(x_end * CHROMA_SIDE - 1, y * CHROMA_SIDE + row)
                    }),
                ],
                tokens: tokens.left(),
                modes: modes.left_modes(),
            };
            sender
                .send(boundary)
                .expect("the right strip stopped listening");
        }
    }

    StripResult { macroblocks, spool }
}

struct LumaCandidate {
    y2: [i16; 16],
    y: [[i16; 16]; 16],
    pixels: [u8; 256],
}

struct ChromaCandidate {
    levels: [[i16; 16]; 4],
    pixels: [u8; 64],
    sse: u64,
}

fn luma_block_samples(
    source: &YuvPlanes,
    (macroblock_x, macroblock_y): (usize, usize),
) -> [[u8; 16]; 16] {
    let stride = source.y_stride;
    let x = macroblock_x * MACROBLOCK_SIDE;
    let y = macroblock_y * MACROBLOCK_SIDE;
    std::array::from_fn(|block| {
        let block_x = (block % 4) * 4;
        let block_y = (block / 4) * 4;
        sample_block(&source.y, stride, x + block_x, y + block_y)
    })
}

fn chroma_block_samples(
    source: &YuvPlanes,
    (macroblock_x, macroblock_y): (usize, usize),
) -> [[u8; 64]; 2] {
    let stride = source.chroma_stride;
    let x = macroblock_x * CHROMA_SIDE;
    let y = macroblock_y * CHROMA_SIDE;
    [&source.u, &source.v].map(|plane| plane_block(plane, stride, x, y))
}

/// Copy one 8×8 plane into its contiguous lane layout. Each source row is contiguous, so
/// this costs eight 8-byte copies rather than four 4×4-block gathers.
fn plane_block(plane: &[u8], stride: usize, x: usize, y: usize) -> [u8; 64] {
    let mut block = [0u8; 64];
    for (row, pixels) in block.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let start = (y + row) * stride + x;
        pixels.copy_from_slice(&plane[start..start + 8]);
    }
    block
}

/// One intra16 mode's prediction and the forward transforms derived from it.
struct LumaPrediction {
    pixels: [u8; 256],
    coefficients: [[i32; 16]; 16],
    dc_coefficients: [i32; 16],
}

impl LumaPrediction {
    fn new<S: Simd>(
        simd: S,
        samples: &[[u8; 16]; 16],
        edges: &predict::Edges<16>,
        mode: u8,
    ) -> Self {
        let pixels = predict::luma_block(mode, edges);
        let mut coefficients = [[0i32; 16]; 16];
        let mut dc_grid = [0i32; 16];
        for (block, samples) in samples.iter().enumerate() {
            let block_x = (block % 4) * 4;
            let block_y = (block / 4) * 4;
            let prediction = sample_block(&pixels, 16, block_x, block_y);
            let transformed = residual_coefficients(simd, samples, &prediction);
            dc_grid[block] = transformed[0];
            coefficients[block] = transformed;
        }
        Self {
            pixels,
            coefficients,
            dc_coefficients: transform::forward_wht(&dc_grid),
        }
    }
}

/// One chroma mode's two predictions and the batched forward transforms derived from them.
struct ChromaPrediction<S: Simd> {
    pixels: [[u8; 64]; 2],
    coefficients: [transform::BlockLanes<S>; 2],
}

impl<S: Simd> ChromaPrediction<S> {
    fn new(simd: S, samples: &[[u8; 64]; 2], edges: &[predict::Edges<8>; 2], mode: u8) -> Self {
        let pixels = [
            predict::chroma_block(mode, &edges[0]),
            predict::chroma_block(mode, &edges[1]),
        ];
        let coefficients = std::array::from_fn(|plane| {
            transform::forward_dct_8x8_lanes(simd, &samples[plane], &pixels[plane])
        });
        Self {
            pixels,
            coefficients,
        }
    }
}

/// The selected chroma mode and the prediction its levels were quantized against.
struct ChromaWinner<S: Simd> {
    mode: u8,
    u: ChromaCandidate,
    v: ChromaCandidate,
    prediction: ChromaPrediction<S>,
}

/// Both quantizers reconstruct against the prediction that produced these coefficients.
#[derive(Clone, Copy)]
struct BlockPrediction {
    mode: u8,
    pixels: [u8; 16],
    coefficients: [i32; 16],
}

struct BlockCandidate {
    prediction: BlockPrediction,
    levels: [i16; 16],
    pixels: [u8; 16],
    sse: u64,
    score: u64,
    mode_cost: u64,
    rate: u64,
}

struct I4Candidate {
    levels: [[i16; 16]; 16],
    modes: [u8; 16],
    pixels: [u8; 256],
    sse: u64,
    // Sum of the selected blocks' token rates for the subblock plane.
    rate: u64,
}

fn encode_macroblock<S: Simd>(
    source: &YuvPlanes,
    reconstructed: &mut Reconstruction,
    quantizer: &Quantizer<S>,
    tokens: &TokenContexts,
    contexts: &vp8::BmodeContexts,
    (macroblock_x, macroblock_y): (usize, usize),
    i4_first: bool,
) -> DenseMacroblock {
    let lambda = quantizer.lambda;
    let neighbors = tokens.contexts(macroblock_x);
    let (zero_chroma_rate, zero_luma_rate) = if lambda == 0 {
        (0, 0)
    } else {
        let mut empty_neighbors = neighbors;
        let chroma = empty_neighbors.chroma_cost(&quantizer.costs, &[[0; 16]; 4], &[[0; 16]; 4]);
        // Chroma retains the zero-I16 luma rate inside rounding, even for I4 macroblocks.
        let luma = empty_neighbors.luma_cost(&quantizer.costs, Some(&[0; 16]), &[[0; 16]; 16]);
        (chroma, luma)
    };
    let luma_edges = predict::Edges::luma(&reconstructed.y, macroblock_x, macroblock_y);
    let luma_samples = luma_block_samples(source, (macroblock_x, macroblock_y));
    let mut luma_prediction = LumaPrediction::new(quantizer.simd, &luma_samples, &luma_edges, DC);
    let (mut best_luma, mut best_luma_score) = analyze_luma(
        &luma_samples,
        &luma_prediction,
        quantizer,
        neighbors,
        Quantization::Nearest,
        u64::MAX,
        zero_chroma_rate + vp8::i16_mode_cost(DC),
    )
    .expect("the first luma mode has no score limit");
    let mut luma_mode = DC;
    for mode in [VERTICAL, HORIZONTAL, TRUE_MOTION] {
        let prediction = LumaPrediction::new(quantizer.simd, &luma_samples, &luma_edges, mode);
        if let Some((candidate, score)) = analyze_luma(
            &luma_samples,
            &prediction,
            quantizer,
            neighbors,
            Quantization::Nearest,
            best_luma_score,
            zero_chroma_rate + vp8::i16_mode_cost(mode),
        ) {
            best_luma = candidate;
            best_luma_score = score;
            luma_mode = mode;
            luma_prediction = prediction;
        }
    }

    let i4;
    if i4_first {
        i4 = analyze_i4(
            &luma_samples,
            reconstructed,
            quantizer,
            neighbors,
            contexts,
            (macroblock_x, macroblock_y),
            best_luma_score,
        )
        .map(|i4| {
            let score = quantizer.score(i4.sse, || {
                i4.rate + zero_chroma_rate + contexts.i4_mode_cost(macroblock_x, &i4.modes)
            });
            (i4, score)
        });
        if lambda != 0 {
            // The subblock score bounds the refinement. An exact tie still favours the
            // refined intra16 candidate, so the refinement only needs to beat it strictly.
            let limit = i4.as_ref().map_or(best_luma_score, |(_, score)| {
                best_luma_score.min(score.saturating_add(1))
            });
            let refined = analyze_luma(
                &luma_samples,
                &luma_prediction,
                quantizer,
                neighbors,
                Quantization::Trellis,
                limit,
                zero_chroma_rate + vp8::i16_mode_cost(luma_mode),
            );
            if let Some((refined, score)) = refined
                && score < best_luma_score
            {
                best_luma = refined;
                best_luma_score = score;
            }
        }
    } else {
        if lambda != 0 {
            // The winning nearest pass already derived this mode's transforms.
            let refined = analyze_luma(
                &luma_samples,
                &luma_prediction,
                quantizer,
                neighbors,
                Quantization::Trellis,
                best_luma_score,
                zero_chroma_rate + vp8::i16_mode_cost(luma_mode),
            );
            if let Some((refined, score)) = refined
                && score < best_luma_score
            {
                best_luma = refined;
                best_luma_score = score;
            }
        }
        i4 = analyze_i4(
            &luma_samples,
            reconstructed,
            quantizer,
            neighbors,
            contexts,
            (macroblock_x, macroblock_y),
            best_luma_score,
        )
        .map(|i4| {
            let score = quantizer.score(i4.sse, || {
                i4.rate + zero_chroma_rate + contexts.i4_mode_cost(macroblock_x, &i4.modes)
            });
            (i4, score)
        });
    }

    let mut selected = (
        Luma::Intra16 {
            mode: luma_mode,
            dc: best_luma.y2,
        },
        best_luma.pixels,
        best_luma.y,
    );
    if let Some((i4, score)) = i4
        && score < best_luma_score
    {
        selected = (Luma::Intra4 { modes: i4.modes }, i4.pixels, i4.levels);
    }
    let (luma, pixels, y) = selected;
    commit_pixels(
        &mut reconstructed.y,
        macroblock_x * MACROBLOCK_SIDE,
        macroblock_y * MACROBLOCK_SIDE,
        16,
        &pixels,
    );

    let chroma_edges = [
        predict::Edges::chroma(&reconstructed.u, macroblock_x, macroblock_y),
        predict::Edges::chroma(&reconstructed.v, macroblock_x, macroblock_y),
    ];
    let chroma_samples = chroma_block_samples(source, (macroblock_x, macroblock_y));
    let mut best_chroma: Option<ChromaWinner<S>> = None;
    let mut best_chroma_score = u64::MAX;
    for mode in [DC, VERTICAL, HORIZONTAL, TRUE_MOTION] {
        let prediction =
            ChromaPrediction::new(quantizer.simd, &chroma_samples, &chroma_edges, mode);
        let [u, v] = analyze_chroma(
            &chroma_samples,
            &prediction,
            quantizer,
            neighbors,
            Quantization::Nearest,
        );
        if (u.sse + v.sse) * CHROMA_SAMPLE_AREA * 256
            + lambda * (zero_luma_rate + vp8::chroma_mode_cost(mode)) / 256
            >= best_chroma_score
        {
            continue;
        }
        let score = quantizer.chroma_score(&u, &v, neighbors, zero_luma_rate, mode);
        if score < best_chroma_score {
            best_chroma = Some(ChromaWinner {
                mode,
                u,
                v,
                prediction,
            });
            best_chroma_score = score;
        }
    }
    let ChromaWinner {
        mode: chroma_mode,
        u: mut best_u,
        v: mut best_v,
        prediction,
    } = best_chroma.expect("four chroma modes were tried");
    if lambda != 0 {
        let [u, v] = analyze_chroma(
            &chroma_samples,
            &prediction,
            quantizer,
            neighbors,
            Quantization::Trellis,
        );
        if quantizer.chroma_score(&u, &v, neighbors, zero_luma_rate, chroma_mode)
            < best_chroma_score
        {
            best_u = u;
            best_v = v;
        }
    }
    commit_pixels(
        &mut reconstructed.u,
        macroblock_x * CHROMA_SIDE,
        macroblock_y * CHROMA_SIDE,
        8,
        &best_u.pixels,
    );
    commit_pixels(
        &mut reconstructed.v,
        macroblock_x * CHROMA_SIDE,
        macroblock_y * CHROMA_SIDE,
        8,
        &best_v.pixels,
    );
    DenseMacroblock {
        luma,
        y,
        chroma_mode,
        u: best_u.levels,
        v: best_v.levels,
    }
}

fn analyze_i4<S: Simd>(
    samples: &[[u8; 16]; 16],
    reconstructed: &Reconstruction,
    quantizer: &Quantizer<S>,
    mut neighbors: MacroblockContexts,
    contexts: &vp8::BmodeContexts,
    (macroblock_x, macroblock_y): (usize, usize),
    score_limit: u64,
) -> Option<I4Candidate> {
    let frame_stride = reconstructed.y.frame_stride();
    let x = macroblock_x * MACROBLOCK_SIDE;
    let y = macroblock_y * MACROBLOCK_SIDE;
    let mut levels = [[0i16; 16]; 16];
    let mut modes = [0u8; 16];
    let mut pixels = [0u8; 256];
    let mut sse = 0u64;
    let mut rate = 0u64;
    let mut score = 0u64;

    for block in 0..16 {
        let block_x = block % 4;
        let block_y = block / 4;
        let block_at_x = x + block_x * 4;
        let block_at_y = y + block_y * 4;
        let sample = |column: usize, row: usize| {
            if column >= x && column < x + MACROBLOCK_SIDE && row >= y && row < y + MACROBLOCK_SIDE
            {
                pixels[(row - y) * 16 + column - x]
            } else {
                reconstructed.y.sample(column, row)
            }
        };
        let corner = if block_at_y == 0 {
            127
        } else if block_at_x == 0 {
            129
        } else {
            sample(block_at_x - 1, block_at_y - 1)
        };
        let mut left = [129; 4];
        if block_at_x > 0 {
            for (row, value) in left.iter_mut().enumerate() {
                *value = sample(block_at_x - 1, block_at_y + row);
            }
        }
        let mut top = [127; 8];
        if block_at_y > 0 {
            for (column, value) in top[..4].iter_mut().enumerate() {
                *value = sample(block_at_x + column, block_at_y - 1);
            }
            // Rightmost subblocks retain the macroblock's original above-right edge.
            let above_right_y = if block_x == 3 {
                y.checked_sub(1)
            } else {
                Some(block_at_y - 1)
            };
            if let Some(row) = above_right_y {
                for (column, value) in top[4..].iter_mut().enumerate() {
                    *value = sample((block_at_x + 4 + column).min(frame_stride - 1), row);
                }
            }
        }
        let mut best: Option<BlockCandidate> = None;
        let block_samples = &samples[block];
        let borders = predict::Borders::new(&top, &left, corner);

        for mode in [B_DC, B_TM, B_VE, B_HE, B_LD, B_RD, B_VR, B_VL, B_HD, B_HU] {
            let limit = best.as_ref().map_or(u64::MAX, |previous| previous.score);
            let mode_cost = contexts.mode_cost(macroblock_x, block, &modes, mode);
            // The mode's cost is a floor on its score: every residual rate is nonnegative.
            if quantizer.lambda * mode_cost / 256 >= limit {
                continue;
            }
            let pixels = borders.block(mode);
            let prediction = BlockPrediction {
                mode,
                pixels,
                coefficients: residual_coefficients(quantizer.simd, block_samples, &pixels),
            };
            if let Some(candidate) = analyze_subblock(
                quantizer,
                block_samples,
                &prediction,
                mode_cost,
                neighbors.y(block),
                Quantization::Nearest,
                None,
                limit,
            ) {
                best = Some(candidate);
            }
        }

        let mut candidate = best.expect("ten subblock modes were tried");
        // The winning nearest mode was scored under these same contexts; its own cost is
        // still a floor on any trellis refinement of it.
        if quantizer.lambda != 0
            && quantizer.lambda * candidate.mode_cost / 256 < candidate.score
            && let Some(refined) = analyze_subblock(
                quantizer,
                block_samples,
                &candidate.prediction,
                candidate.mode_cost,
                neighbors.y(block),
                Quantization::Trellis,
                Some(&candidate.levels),
                candidate.score,
            )
        {
            candidate = refined;
        }
        // Remaining distortion and token costs cannot reduce this completed-block bound.
        score += candidate.score;
        if score >= score_limit {
            return None;
        }
        modes[block] = candidate.prediction.mode;
        levels[block] = candidate.levels;
        neighbors.set_y(block, candidate.levels.iter().any(|level| *level != 0));
        sse += candidate.sse;
        rate += candidate.rate;
        copy_block(&mut pixels, 16, block_x * 4, block_y * 4, &candidate.pixels);
    }

    Some(I4Candidate {
        levels,
        modes,
        pixels,
        sse,
        rate,
    })
}

#[allow(clippy::too_many_arguments)]
fn analyze_subblock<S: Simd>(
    quantizer: &Quantizer<S>,
    samples: &[u8; 16],
    prediction: &BlockPrediction,
    mode_cost: u64,
    context: usize,
    method: Quantization,
    previous_levels: Option<&[i16; 16]>,
    score_limit: u64,
) -> Option<BlockCandidate> {
    let levels = quantizer.block(
        &prediction.coefficients,
        CoefficientPlane::Luma,
        context,
        method,
    );
    // Equal levels score exactly like the candidate they came from, so a refinement (which
    // must strictly improve) can stop before reconstructing and rewinding its rate.
    if previous_levels.is_some_and(|previous| previous == &levels) {
        return None;
    }
    let coefficients =
        quantize::dequantize_block(&levels, quantizer.factors.y_dc, quantizer.factors.y_ac);
    let residual = transform::inverse_dct(quantizer.simd, &coefficients);
    let (pixels, sse) =
        transform::reconstruct_sse(quantizer.simd, samples, &prediction.pixels, &residual);
    // Residual rate is nonnegative; preserve the combined rounding only for the full score.
    if sse * 256 + quantizer.lambda * mode_cost / 256 >= score_limit {
        return None;
    }
    // The macroblock's exact score reuses this rate instead of walking the block again.
    let rate = if quantizer.lambda == 0 {
        0
    } else {
        quantizer
            .costs
            .block(&levels, CoefficientPlane::Luma, context)
    };
    let score = quantizer.score(sse, || mode_cost + rate);
    (score < score_limit).then_some(BlockCandidate {
        prediction: *prediction,
        levels,
        pixels,
        sse,
        score,
        mode_cost,
        rate,
    })
}

/// Quantizes one intra16 prediction, returning its exact score when that score beats
/// `score_limit`. Distortion and rate are accumulated as blocks complete, so a mode that
/// cannot win stops before quantizing the rest.
fn analyze_luma<S: Simd>(
    samples: &[[u8; 16]; 16],
    prediction: &LumaPrediction,
    quantizer: &Quantizer<S>,
    mut neighbors: MacroblockContexts,
    method: Quantization,
    score_limit: u64,
    fixed_rate: u64,
) -> Option<(LumaCandidate, u64)> {
    let factors = quantizer.factors;

    let y2_levels = quantizer.block(
        &prediction.dc_coefficients,
        CoefficientPlane::LumaDc,
        neighbors.y2(),
        method,
    );
    let y2 = transform::inverse_wht(&quantize::dequantize_block(
        &y2_levels,
        factors.y2_dc,
        factors.y2_ac,
    ));

    let mut rate = if quantizer.lambda == 0 {
        0
    } else {
        quantizer
            .costs
            .block(&y2_levels, CoefficientPlane::LumaDc, neighbors.y2())
    };
    let mut levels = [[0i16; 16]; 16];
    let mut pixels = [0u8; 256];
    let mut sse = 0u64;
    let mut score = 0;
    for (block, samples) in samples.iter().enumerate() {
        let block_x = (block % 4) * 4;
        let block_y = (block / 4) * 4;
        let mut ac = prediction.coefficients[block];
        ac[0] = 0;
        let context = neighbors.y(block);
        let block_levels = quantizer.block(&ac, CoefficientPlane::LumaAc, context, method);
        levels[block] = block_levels;
        neighbors.set_y(block, block_levels.iter().any(|level| *level != 0));
        let mut coefficients =
            quantize::dequantize_block(&block_levels, factors.y_dc, factors.y_ac);
        coefficients[0] = y2[block];
        let residual = transform::inverse_dct(quantizer.simd, &coefficients);
        let predicted = sample_block(&prediction.pixels, 16, block_x, block_y);
        let (rebuilt, error) =
            transform::reconstruct_sse(quantizer.simd, samples, &predicted, &residual);
        copy_block(&mut pixels, 16, block_x, block_y, &rebuilt);
        sse += error;
        if quantizer.lambda != 0 {
            rate += quantizer
                .costs
                .block(&block_levels, CoefficientPlane::LumaAc, context);
        }
        // Remaining distortion and token rate cannot lower a completed-block score.
        score = sse * 256 + quantizer.lambda * (rate + fixed_rate) / 256;
        if score >= score_limit {
            return None;
        }
    }

    Some((
        LumaCandidate {
            y2: y2_levels,
            y: levels,
            pixels,
        },
        score,
    ))
}

fn analyze_chroma<S: Simd>(
    samples: &[[u8; 64]; 2],
    prediction: &ChromaPrediction<S>,
    quantizer: &Quantizer<S>,
    mut neighbors: MacroblockContexts,
    method: Quantization,
) -> [ChromaCandidate; 2] {
    let matrix = &quantizer.matrices[2];
    std::array::from_fn(|plane| {
        let candidate_levels;
        let levels = match method {
            Quantization::Nearest => {
                let levels = matrix.quantize_lanes(quantizer.simd, &prediction.coefficients[plane]);
                candidate_levels = transform::block_levels(&levels);
                levels
            }
            Quantization::Trellis => {
                // The trellis context chain depends on the previous block's levels, so the
                // four blocks quantize in order; dequantization onward is batched again.
                let coefficients = transform::block_coefficients(&prediction.coefficients[plane]);
                let mut block_levels = [[0i16; 16]; 4];
                for block in 0..4 {
                    let context = if plane == 0 {
                        neighbors.u(block)
                    } else {
                        neighbors.v(block)
                    };
                    block_levels[block] = quantizer.block(
                        &coefficients[block],
                        CoefficientPlane::Chroma,
                        context,
                        method,
                    );
                    let nonzero = block_levels[block].iter().any(|level| *level != 0);
                    if plane == 0 {
                        neighbors.set_u(block, nonzero);
                    } else {
                        neighbors.set_v(block, nonzero);
                    }
                }
                candidate_levels = block_levels;
                transform::levels_to_lanes(quantizer.simd, &block_levels)
            }
        };
        let dequantized = matrix.dequantize_lanes(quantizer.simd, &levels);
        let residual = transform::inverse_dct_lanes(quantizer.simd, &dequantized);
        let (pixels, block_sse) = transform::reconstruct_8x8(
            quantizer.simd,
            &samples[plane],
            &prediction.pixels[plane],
            &residual,
        );
        ChromaCandidate {
            levels: candidate_levels,
            pixels,
            sse: block_sse.into_iter().sum(),
        }
    })
}

fn sample_block(plane: &[u8], stride: usize, x: usize, y: usize) -> [u8; 16] {
    let mut block = [0u8; 16];
    for (row, chunk) in block.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let start = (y + row) * stride + x;
        chunk.copy_from_slice(&plane[start..start + 4]);
    }
    block
}

fn copy_block(plane: &mut [u8], stride: usize, x: usize, y: usize, pixels: &[u8; 16]) {
    for (row, pixels) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let start = (y + row) * stride + x;
        plane[start..start + 4].copy_from_slice(pixels);
    }
}

fn residual_coefficients<S: Simd>(simd: S, samples: &[u8; 16], prediction: &[u8; 16]) -> [i32; 16] {
    let residual =
        std::array::from_fn(|index| i32::from(samples[index]) - i32::from(prediction[index]));
    transform::forward_dct(simd, &residual)
}

fn commit_pixels(plane: &mut Plane, x: usize, y: usize, side: usize, pixels: &[u8]) {
    for (row, pixels) in pixels.chunks_exact(side).enumerate() {
        plane.write(y + row, x, pixels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_probability_check_is_gated() {
        assert!(should_check_exactly(0, 4096, 4096));
        assert!(should_check_exactly(SMALL_FIT_SAVINGS, 4096, 4096));
        assert!(!should_check_exactly(SMALL_FIT_SAVINGS + 1, 4096, 4096));
        assert!(should_check_exactly(1024 * 256, 100, 100));
        assert!(!should_check_exactly(1024 * 256, 128, 128));
    }

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

    fn compare_activity<S: Simd>(simd: S) {
        let flat = [128u8, 128, 128, 255].repeat(64 * 64);
        let checker: Vec<u8> = (0..64 * 64)
            .flat_map(|index| {
                let value = if (index % 64 / 2 + index / 64 / 2) % 2 == 0 {
                    255
                } else {
                    0
                };
                [value, value, value, 255]
            })
            .collect();
        assert!(!i4_first(&color::from_rgba(simd, &flat, 64, 64)));
        assert!(i4_first(&color::from_rgba(simd, &checker, 64, 64)));
    }

    #[test]
    fn activity_classifier_separates_flat_from_checker() {
        dispatch!(Level::new(), simd => compare_activity(simd));
    }

    #[test]
    fn flat_block_retains_its_color() {
        let rgba = [120u8, 60, 200, 255].repeat(16 * 16);
        let webp = encode_rgba(&rgba, 16, 16, 90).unwrap();
        let decoded = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP)
            .unwrap()
            .to_rgba8();
        let pixel = decoded.get_pixel(0, 0).0;
        for (got, want) in pixel.iter().zip([120u8, 60, 200]) {
            assert!(got.abs_diff(want) <= 8, "{pixel:?}");
        }
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
    fn strip_encoding_matches_serial_bytes() {
        for (width, height) in [
            (16, 16),
            (32, 32),
            (65, 65),
            (129, 17),
            (129, 16),
            (257, 193),
        ] {
            let rgba: Vec<u8> = (0..height)
                .flat_map(|y| {
                    (0..width).flat_map(move |x| {
                        [
                            (x * 37) as u8,
                            (y * 53) as u8,
                            ((x + y) * 29) as u8,
                            (x * 17 + y * 31) as u8,
                        ]
                    })
                })
                .collect();
            let simd = fearless_simd::Fallback::new();
            let serial = encode_frame_from_rgba_with_strips(
                simd,
                &rgba,
                width,
                height,
                75,
                TrellisMode::Exact,
                1,
            )
            .unwrap();
            let stripped = encode_frame_from_rgba_with_strips(
                simd,
                &rgba,
                width,
                height,
                75,
                TrellisMode::Exact,
                2,
            )
            .unwrap();
            assert_eq!(serial, stripped, "{width}x{height}");
        }
    }

    struct ScalarChroma {
        pixels: [[u8; 64]; 2],
        coefficients: [[[i32; 16]; 4]; 2],
        candidates: [ChromaCandidate; 2],
    }

    /// The pre-batch per-block pipeline: sample each subblock, transform, quantize,
    /// dequantize, inverse-transform, and reconstruct. It is the oracle for the lane kernel.
    fn scalar_chroma_reference<S: Simd>(
        quantizer: &Quantizer<S>,
        samples: &[[u8; 64]; 2],
        edges: &[predict::Edges<8>; 2],
        mode: u8,
        mut neighbors: MacroblockContexts,
        method: Quantization,
    ) -> ScalarChroma {
        let pixels = [
            predict::chroma_block(mode, &edges[0]),
            predict::chroma_block(mode, &edges[1]),
        ];
        let mut coefficients = [[[0i32; 16]; 4]; 2];
        let candidates = std::array::from_fn(|plane| {
            let mut levels = [[0i16; 16]; 4];
            let mut rebuilt = [0u8; 64];
            let mut sse = 0u64;
            for block in 0..4 {
                let block_x = (block % 2) * 4;
                let block_y = (block / 2) * 4;
                let predicted = sample_block(&pixels[plane], 8, block_x, block_y);
                let block_samples = sample_block(&samples[plane], 8, block_x, block_y);
                coefficients[plane][block] =
                    residual_coefficients(quantizer.simd, &block_samples, &predicted);
                let context = if plane == 0 {
                    neighbors.u(block)
                } else {
                    neighbors.v(block)
                };
                levels[block] = quantizer.block(
                    &coefficients[plane][block],
                    CoefficientPlane::Chroma,
                    context,
                    method,
                );
                let nonzero = levels[block].iter().any(|level| *level != 0);
                if plane == 0 {
                    neighbors.set_u(block, nonzero);
                } else {
                    neighbors.set_v(block, nonzero);
                }
                let dequantized = quantize::dequantize_block(
                    &levels[block],
                    quantizer.factors.chroma_dc,
                    quantizer.factors.chroma_ac,
                );
                let residual = transform::inverse_dct(quantizer.simd, &dequantized);
                let (block_pixels, error) = transform::reconstruct_sse(
                    quantizer.simd,
                    &block_samples,
                    &predicted,
                    &residual,
                );
                copy_block(&mut rebuilt, 8, block_x, block_y, &block_pixels);
                sse += error;
            }
            ChromaCandidate {
                levels,
                pixels: rebuilt,
                sse,
            }
        });
        ScalarChroma {
            pixels,
            coefficients,
            candidates,
        }
    }

    fn boundary_chroma_samples(sequence: &mut u32, trial: usize) -> [[u8; 64]; 2] {
        std::array::from_fn(|plane| {
            std::array::from_fn(|index| match trial {
                0 => 0,
                1 => 255,
                2 => u8::from((index / 8 + index % 8 + plane) % 2 == 0).wrapping_mul(255),
                3 => ((index * 37 + plane * 11) % 256) as u8,
                4 => {
                    if index == 0 {
                        255
                    } else if index == 63 {
                        0
                    } else {
                        128
                    }
                }
                _ => {
                    *sequence ^= *sequence << 13;
                    *sequence ^= *sequence >> 17;
                    *sequence ^= *sequence << 5;
                    (*sequence >> 24) as u8
                }
            })
        })
    }

    fn check_chroma_batches<S: Simd>(simd: S) {
        let mut reconstructed = Reconstruction::new(64, 32, 2);
        for row in 0..16 {
            for (column, sample) in reconstructed.u.row_mut(row).iter_mut().enumerate() {
                *sample = ((row * 31 + column * 7 + 3) % 256) as u8;
            }
            for (column, sample) in reconstructed.v.row_mut(row).iter_mut().enumerate() {
                *sample = ((row * 17 + column * 29 + 11) % 256) as u8;
            }
        }
        let mut sequence = 0xc0ff_ee01u32;
        for trial in 0..8 {
            let samples = boundary_chroma_samples(&mut sequence, trial);
            for origin in [(0, 0), (1, 0), (0, 1), (2, 2)] {
                let edges = [
                    predict::Edges::chroma(&reconstructed.u, origin.0, origin.1),
                    predict::Edges::chroma(&reconstructed.v, origin.0, origin.1),
                ];
                for mode in [DC, VERTICAL, HORIZONTAL, TRUE_MOTION] {
                    for (index, lambda) in [
                        (0u8, 0u64),
                        (0, 16),
                        (63, 16 * 63 * 63),
                        (127, 16 * 127 * 127),
                    ] {
                        let quantizer = Quantizer::new(
                            simd,
                            quantize::factors(index),
                            lambda,
                            TrellisMode::Exact,
                        );
                        for method in [Quantization::Nearest, Quantization::Trellis] {
                            let mut neighbors = MacroblockContexts::default();
                            neighbors.set_u(0, trial % 3 == 0);
                            neighbors.set_u(1, trial % 2 == 0);
                            neighbors.set_v(0, trial % 4 == 1);
                            neighbors.set_v(1, trial % 3 != 1);
                            let prediction = ChromaPrediction::new(simd, &samples, &edges, mode);
                            let actual = analyze_chroma(
                                &samples,
                                &prediction,
                                &quantizer,
                                neighbors,
                                method,
                            );
                            let expected = scalar_chroma_reference(
                                &quantizer, &samples, &edges, mode, neighbors, method,
                            );
                            assert_eq!(prediction.pixels, expected.pixels);
                            for (plane, actual_plane) in actual.iter().enumerate() {
                                assert_eq!(
                                    transform::block_coefficients(&prediction.coefficients[plane]),
                                    expected.coefficients[plane],
                                    "trial {trial} origin {origin:?} mode {mode} q{index} {method:?} plane {plane}"
                                );
                                assert_eq!(actual_plane.levels, expected.candidates[plane].levels);
                                assert_eq!(actual_plane.pixels, expected.candidates[plane].pixels);
                                assert_eq!(actual_plane.sse, expected.candidates[plane].sse);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn chroma_batches_match_scalar_path() {
        check_chroma_batches(fearless_simd::Fallback::new());
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        dispatch!(Level::baseline(), simd => check_chroma_batches(simd));
        dispatch!(Level::new(), simd => check_chroma_batches(simd));
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
            let source = color::from_rgba(fearless_simd::Fallback::new(), &rgba, width, height);
            for quality in [0, 25, 75, 100] {
                let quantizer = quantize::quantizer_index(quality);
                let factors = quantize::factors(quantizer);
                let lambda = 16 * u64::from(quantizer).pow(2);
                let mut analysis = Macroblocks::new(
                    fearless_simd::Fallback::new(),
                    &source,
                    factors,
                    lambda,
                    TrellisMode::Exact,
                );
                let mut macroblocks = Vec::with_capacity(analysis.len());
                let mut expected = [
                    vec![0; source.y.len()],
                    vec![0; source.u.len()],
                    vec![0; source.v.len()],
                ];
                while let Some(macroblock) = analysis.next() {
                    let x = macroblocks.len() % analysis.columns;
                    let y = macroblocks.len() / analysis.columns;
                    for ((plane, side), pixels) in [
                        (&analysis.reconstructed.y, 16),
                        (&analysis.reconstructed.u, 8),
                        (&analysis.reconstructed.v, 8),
                    ]
                    .into_iter()
                    .zip(&mut expected)
                    {
                        for row in 0..side {
                            let start = (y * side + row) * plane.stride() + x * side;
                            pixels[start..start + side].copy_from_slice(
                                &plane.row(y * side + row)[x * side..(x + 1) * side],
                            );
                        }
                    }
                    macroblocks.push(macroblock);
                }
                let spool = analysis.into_coefficient_spool();
                // Disabling the loop filter isolates predictor and coefficient reconstruction.
                let frame =
                    encode_frame(width, height, quantizer, 0, &macroblocks, &spool).unwrap();
                let decoded =
                    image_webp::vp8::Vp8Decoder::decode_frame(std::io::Cursor::new(frame)).unwrap();
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
