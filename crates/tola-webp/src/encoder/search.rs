//! VP8 intra prediction search and reconstruction for one macroblock.

use fearless_simd::Simd;

use crate::TrellisMode;
use crate::color::{RgbaFrame, YuvRow};
use crate::macroblock::{CHROMA_SIDE, DenseMacroblock, Luma, MACROBLOCK_SIDE};
use crate::predict::{
    self, B_DC, B_HD, B_HE, B_HU, B_LD, B_RD, B_TM, B_VE, B_VL, B_VR, DC, HORIZONTAL, TRUE_MOTION,
    VERTICAL,
};
use crate::quantize::{self, Factors};
use crate::reconstruction::{Plane, Reconstruction};
use crate::token::{CoefficientPlane, MacroblockContexts, TokenCosts};
use crate::{transform, trellis, vp8};

// Each YUV420 chroma sample represents a 2×2 luma cell.
const CHROMA_SAMPLE_AREA: u64 = 4;
const SUBBLOCK_SIDE: usize = 4;
const BLOCKS_PER_ROW: usize = MACROBLOCK_SIDE / SUBBLOCK_SIDE;

pub(super) struct Quantizer<S> {
    pub(super) simd: S,
    factors: Factors,
    luma: quantize::Matrix,
    luma_dc: quantize::Matrix,
    chroma: quantize::Matrix,
    trellis: trellis::Quantizer,
    costs: TokenCosts,
    lambda: u64,
}

#[derive(Clone, Copy)]
enum Quantization {
    Nearest,
    Trellis,
}

impl<S: Simd> Quantizer<S> {
    pub(super) fn new(simd: S, factors: Factors, lambda: u64, mode: TrellisMode) -> Self {
        let costs = TokenCosts::default();
        Self {
            simd,
            factors,
            luma: quantize::Matrix::new(factors.y_dc, factors.y_ac),
            luma_dc: quantize::Matrix::new(factors.y2_dc, factors.y2_ac),
            chroma: quantize::Matrix::new(factors.chroma_dc, factors.chroma_ac),
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

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn fork(&self) -> Self {
        Self {
            simd: self.simd,
            factors: self.factors,
            luma: self.luma,
            luma_dc: self.luma_dc,
            chroma: self.chroma,
            trellis: self.trellis.fork(),
            costs: self.costs.clone(),
            lambda: self.lambda,
        }
    }

    fn block(
        &self,
        coefficients: &[i32; 16],
        plane: CoefficientPlane,
        context: usize,
        method: Quantization,
    ) -> [i16; 16] {
        match method {
            Quantization::Nearest => {
                let matrix = match plane {
                    CoefficientPlane::LumaAc | CoefficientPlane::Luma => &self.luma,
                    CoefficientPlane::LumaDc => &self.luma_dc,
                    CoefficientPlane::Chroma => &self.chroma,
                };
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
}

/// Mean 4x4 luma activity above which the intra4 search is expected to win often enough
/// to run first. Both orders use exact bounds, so this only changes how much work the
/// macroblock search performs; selected modes and output bytes are unaffected.
///
/// Calibrated between the smooth photograph (about 970) and gallery-1 (about 2840), with
/// enough headroom that smooth sources stay on the intra16-first order.
const INTRA4_ACTIVITY_THRESHOLD: u64 = 1200;

/// Macroblock rows and columns sampled when classifying source activity.
const ACTIVITY_GRID: usize = 8;

/// Whether this source is textured enough to search intra4 before intra16.
///
/// Integer content measure over sampled macroblocks; independent of quality and timing.
pub(super) fn prefers_intra4<S: Simd>(simd: S, source: RgbaFrame<'_>) -> bool {
    let stride = MACROBLOCK_SIDE;
    let columns = source.columns();
    let rows = source.rows();
    let step_x = columns.div_ceil(ACTIVITY_GRID);
    let step_y = rows.div_ceil(ACTIVITY_GRID);
    let mut activity = 0u64;
    let mut samples = 0u64;
    for macroblock_y in (0..rows).step_by(step_y) {
        for macroblock_x in (0..columns).step_by(step_x) {
            let luma = source.luma_block(simd, macroblock_x, macroblock_y);
            for block in 0..BLOCKS_PER_ROW * BLOCKS_PER_ROW {
                let (x, y) = subblock_offset(block);
                let start = y * stride + x;
                for row in 0..SUBBLOCK_SIDE {
                    let row_start = start + row * stride;
                    let pixels = &luma[row_start..row_start + SUBBLOCK_SIDE];
                    for pair in pixels.windows(2) {
                        activity += u64::from(pair[0].abs_diff(pair[1]));
                    }
                }
                for column in 0..SUBBLOCK_SIDE {
                    let mut previous = luma[start + column];
                    for row in 1..SUBBLOCK_SIDE {
                        let value = luma[start + row * stride + column];
                        activity += u64::from(value.abs_diff(previous));
                        previous = value;
                    }
                }
            }
            samples += 1;
        }
    }
    activity / samples >= INTRA4_ACTIVITY_THRESHOLD
}

struct Intra16Candidate {
    dc: [i16; 16],
    ac: [[i16; 16]; 16],
    pixels: [u8; 256],
}

struct ChromaCandidate {
    levels: [[i16; 16]; 4],
    pixels: [u8; 64],
    sse: u64,
}

fn luma_block_samples(source: &YuvRow, column: usize) -> [[u8; 16]; 16] {
    let stride = source.y_stride();
    let x = column * MACROBLOCK_SIDE;
    std::array::from_fn(|block| {
        let (block_x, block_y) = subblock_offset(block);
        sample_block(&source.y, stride, x + block_x, block_y)
    })
}

fn chroma_block_samples(source: &YuvRow, column: usize) -> [[u8; 64]; 2] {
    let stride = source.chroma_stride();
    let x = column * CHROMA_SIDE;
    [&source.u, &source.v].map(|plane| plane_block(plane, stride, x))
}

/// Copy one 8×8 plane into its contiguous lane layout. Each source row is contiguous, so
/// this costs eight 8-byte copies rather than four 4×4-block gathers.
fn plane_block(plane: &[u8], stride: usize, x: usize) -> [u8; 64] {
    let mut block = [0u8; 64];
    for (row, pixels) in block.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let start = row * stride + x;
        pixels.copy_from_slice(&plane[start..start + 8]);
    }
    block
}

/// One intra16 prediction and the transforms retained for its refinement.
#[derive(Default)]
struct Intra16Prediction {
    blocks: [[u8; 16]; 16],
    coefficients: [[i32; 16]; 16],
    dc_coefficients: [i32; 16],
}

impl Intra16Prediction {
    fn predict(&mut self, sample_sums: &[i32; 16], edges: &predict::Edges<16>, mode: u8) {
        let pixels = predict::luma_block(mode, edges);
        let dc_grid = std::array::from_fn(|block| {
            let (x, y) = subblock_offset(block);
            let prediction = sample_block(&pixels, MACROBLOCK_SIDE, x, y);
            let sum = sample_sums[block] - block_sum(&prediction);
            self.blocks[block] = prediction;
            // The forward DCT's DC term is (8 * sum + 7) >> 4, exactly sum >> 1.
            sum >> 1
        });
        self.dc_coefficients = transform::forward_wht(&dc_grid);
    }
}

/// One chroma plane's prediction and batched forward transform.
struct ChromaPrediction<S: Simd> {
    pixels: [u8; 64],
    coefficients: transform::BlockLanes<S>,
}

impl<S: Simd> ChromaPrediction<S> {
    fn new(simd: S, samples: &[u8; 64], edges: &predict::Edges<8>, mode: u8) -> Self {
        let pixels = predict::chroma_block(mode, edges);
        let coefficients = transform::forward_dct_8x8_lanes(simd, samples, &pixels);
        Self {
            pixels,
            coefficients,
        }
    }
}

/// The selected chroma mode and the prediction its levels were quantized against.
struct ChromaWinner<S: Simd> {
    mode: u8,
    planes: [ChromaCandidate; 2],
    predictions: [ChromaPrediction<S>; 2],
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

struct Intra4Candidate {
    levels: [[i16; 16]; 16],
    modes: [u8; 16],
    pixels: [u8; 256],
    sse: u64,
    // Sum of the selected blocks' token rates for the subblock plane.
    rate: u64,
}

pub(super) fn search_macroblock<S: Simd>(
    source: &YuvRow,
    reconstructed: &mut Reconstruction,
    quantizer: &Quantizer<S>,
    neighbors: MacroblockContexts,
    contexts: &vp8::BmodeContexts,
    (macroblock_x, macroblock_y): (usize, usize),
    intra4_first: bool,
) -> DenseMacroblock {
    // Chroma keeps the empty Intra16 luma rate in its rounding, including for Intra4.
    let (zero_luma_rate, zero_chroma_rate) = if quantizer.lambda == 0 {
        (0, 0)
    } else {
        neighbors.empty_rates(&quantizer.costs)
    };
    let column = source.local_column(macroblock_x);
    let position = (macroblock_x, macroblock_y);
    let search = MacroblockSearch {
        quantizer,
        neighbors,
        position,
    };
    let luma = search.luma(
        &luma_block_samples(source, column),
        &reconstructed.y,
        contexts,
        zero_chroma_rate,
        intra4_first,
    );
    commit_pixels(
        &mut reconstructed.y,
        position,
        MACROBLOCK_SIDE,
        &luma.pixels,
    );

    let (chroma_mode, [u, v]) = search.chroma(
        &chroma_block_samples(source, column),
        [&reconstructed.u, &reconstructed.v],
        zero_luma_rate,
    );
    commit_pixels(&mut reconstructed.u, position, CHROMA_SIDE, &u.pixels);
    commit_pixels(&mut reconstructed.v, position, CHROMA_SIDE, &v.pixels);
    DenseMacroblock {
        luma: luma.mode,
        y: luma.levels,
        chroma_mode,
        u: u.levels,
        v: v.levels,
    }
}

struct MacroblockSearch<'a, S> {
    quantizer: &'a Quantizer<S>,
    neighbors: MacroblockContexts,
    position: (usize, usize),
}

struct LumaChoice {
    mode: Luma,
    pixels: [u8; 256],
    levels: [[i16; 16]; 16],
}

impl<S: Simd> MacroblockSearch<'_, S> {
    fn luma(
        &self,
        samples: &[[u8; 16]; 16],
        reconstructed: &Plane,
        contexts: &vp8::BmodeContexts,
        zero_chroma_rate: u64,
        intra4_first: bool,
    ) -> LumaChoice {
        let (macroblock_x, macroblock_y) = self.position;
        let quantizer = self.quantizer;
        let neighbors = self.neighbors;
        let lambda = quantizer.lambda;
        let luma_edges = predict::Edges::luma(reconstructed, macroblock_x, macroblock_y);
        let sample_sums = std::array::from_fn(|block| block_sum(&samples[block]));
        let mut predictions: [Intra16Prediction; 2] = Default::default();
        let mut winner = 0;
        predictions[winner].predict(&sample_sums, &luma_edges, DC);
        let (mut best_luma, mut best_luma_score) = score_intra16(
            samples,
            &mut predictions[winner],
            quantizer,
            neighbors,
            Quantization::Nearest,
            u64::MAX,
            zero_chroma_rate + vp8::i16_mode_cost(DC),
        )
        .expect("the first luma mode has no score limit");
        let mut luma_mode = DC;
        for mode in [VERTICAL, HORIZONTAL, TRUE_MOTION] {
            let fixed_rate = zero_chroma_rate + vp8::i16_mode_cost(mode);
            if quantizer.score(0, || fixed_rate) >= best_luma_score {
                continue;
            }
            let trial = 1 - winner;
            predictions[trial].predict(&sample_sums, &luma_edges, mode);
            if let Some((candidate, score)) = score_intra16(
                samples,
                &mut predictions[trial],
                quantizer,
                neighbors,
                Quantization::Nearest,
                best_luma_score,
                fixed_rate,
            ) {
                best_luma = candidate;
                best_luma_score = score;
                luma_mode = mode;
                winner = trial;
            }
        }

        let score_i4 = |limit: u64| {
            search_intra4(
                samples,
                reconstructed,
                quantizer,
                neighbors,
                contexts,
                self.position,
                limit,
            )
            .map(|i4| {
                let score = quantizer.score(i4.sse, || {
                    i4.rate + zero_chroma_rate + contexts.i4_mode_cost(macroblock_x, &i4.modes)
                });
                (i4, score)
            })
        };
        let mut intra4 = if intra4_first {
            score_i4(best_luma_score)
        } else {
            None
        };
        if lambda != 0 {
            // Intra16 wins an exact tie. When Intra4 was searched first, its score plus one
            // bounds the strict refinement without excluding that tie.
            let limit = intra4.as_ref().map_or(best_luma_score, |(_, score)| {
                best_luma_score.min(score.saturating_add(1))
            });
            if let Some((refined, score)) = score_intra16(
                samples,
                &mut predictions[winner],
                quantizer,
                neighbors,
                Quantization::Trellis,
                limit,
                zero_chroma_rate + vp8::i16_mode_cost(luma_mode),
            ) {
                best_luma = refined;
                best_luma_score = score;
            }
        }
        if !intra4_first {
            intra4 = score_i4(best_luma_score);
        }

        if let Some((intra4, score)) = intra4
            && score < best_luma_score
        {
            return LumaChoice {
                mode: Luma::Intra4 {
                    modes: intra4.modes,
                },
                pixels: intra4.pixels,
                levels: intra4.levels,
            };
        }
        LumaChoice {
            mode: Luma::Intra16 {
                mode: luma_mode,
                dc: best_luma.dc,
            },
            pixels: best_luma.pixels,
            levels: best_luma.ac,
        }
    }

    fn chroma(
        &self,
        samples: &[[u8; 64]; 2],
        reconstructed: [&Plane; 2],
        zero_luma_rate: u64,
    ) -> (u8, [ChromaCandidate; 2]) {
        let (x, y) = self.position;
        let quantizer = self.quantizer;
        let neighbors = self.neighbors;
        let edges = reconstructed.map(|plane| predict::Edges::chroma(plane, x, y));
        let candidate_score = |[u, v]: &[ChromaCandidate; 2], distortion, fixed_rate| {
            quantizer.score(distortion, || {
                let mut neighbors = neighbors;
                fixed_rate + neighbors.chroma_cost(&quantizer.costs, &u.levels, &v.levels)
            })
        };
        let mut best = None;
        let mut best_score = u64::MAX;
        for mode in [DC, VERTICAL, HORIZONTAL, TRUE_MOTION] {
            let fixed_rate = zero_luma_rate + vp8::chroma_mode_cost(mode);
            if quantizer.score(0, || fixed_rate) >= best_score {
                continue;
            }
            let u_prediction = ChromaPrediction::new(quantizer.simd, &samples[0], &edges[0], mode);
            let u = quantize_chroma_plane(
                &samples[0],
                &u_prediction,
                quantizer,
                neighbors,
                0,
                Quantization::Nearest,
            );
            // V's distortion and both token rates are nonnegative.
            if quantizer.score(u.sse * CHROMA_SAMPLE_AREA, || fixed_rate) >= best_score {
                continue;
            }
            let v_prediction = ChromaPrediction::new(quantizer.simd, &samples[1], &edges[1], mode);
            let v = quantize_chroma_plane(
                &samples[1],
                &v_prediction,
                quantizer,
                neighbors,
                1,
                Quantization::Nearest,
            );
            let distortion = (u.sse + v.sse) * CHROMA_SAMPLE_AREA;
            if quantizer.score(distortion, || fixed_rate) >= best_score {
                continue;
            }
            let planes = [u, v];
            let score = candidate_score(&planes, distortion, fixed_rate);
            if score < best_score {
                best = Some(ChromaWinner {
                    mode,
                    planes,
                    predictions: [u_prediction, v_prediction],
                });
                best_score = score;
            }
        }
        let ChromaWinner {
            mode,
            mut planes,
            predictions,
        } = best.expect("four chroma modes were tried");
        if quantizer.lambda != 0 {
            let refined = std::array::from_fn(|plane| {
                quantize_chroma_plane(
                    &samples[plane],
                    &predictions[plane],
                    quantizer,
                    neighbors,
                    plane,
                    Quantization::Trellis,
                )
            });
            let distortion = (refined[0].sse + refined[1].sse) * CHROMA_SAMPLE_AREA;
            let fixed_rate = zero_luma_rate + vp8::chroma_mode_cost(mode);
            if candidate_score(&refined, distortion, fixed_rate) < best_score {
                planes = refined;
            }
        }
        (mode, planes)
    }
}

fn search_intra4<S: Simd>(
    samples: &[[u8; 16]; 16],
    reconstructed: &Plane,
    quantizer: &Quantizer<S>,
    mut neighbors: MacroblockContexts,
    contexts: &vp8::BmodeContexts,
    (macroblock_x, macroblock_y): (usize, usize),
    score_limit: u64,
) -> Option<Intra4Candidate> {
    let x = macroblock_x * MACROBLOCK_SIDE;
    let y = macroblock_y * MACROBLOCK_SIDE;
    let mut levels = [[0i16; 16]; 16];
    let mut modes = [0u8; 16];
    let mut pixels = [0u8; 256];
    let mut sse = 0u64;
    let mut rate = 0u64;
    let mut score = 0u64;

    for block in 0..16 {
        let mut best: Option<BlockCandidate> = None;
        let block_samples = &samples[block];
        let context = neighbors.y(block);
        let borders = subblock_borders(reconstructed, &pixels, (x, y), block);

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
            if let Some(candidate) = analyze_subblock(SubblockInputs {
                quantizer,
                samples: block_samples,
                prediction: &prediction,
                mode_cost,
                context,
                method: Quantization::Nearest,
                previous_levels: None,
                score_limit: limit,
            }) {
                best = Some(candidate);
            }
        }

        let mut candidate = best.expect("ten subblock modes were tried");
        // The winning nearest mode was scored under these same contexts; its own cost is
        // still a floor on any trellis refinement of it.
        if quantizer.lambda != 0
            && quantizer.lambda * candidate.mode_cost / 256 < candidate.score
            && let Some(refined) = analyze_subblock(SubblockInputs {
                quantizer,
                samples: block_samples,
                prediction: &candidate.prediction,
                mode_cost: candidate.mode_cost,
                context,
                method: Quantization::Trellis,
                previous_levels: Some(&candidate.levels),
                score_limit: candidate.score,
            })
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
        neighbors.set_y(block, candidate.levels != [0; 16]);
        sse += candidate.sse;
        rate += candidate.rate;
        store_subblock(&mut pixels, block, &candidate.pixels);
    }

    Some(Intra4Candidate {
        levels,
        modes,
        pixels,
        sse,
        rate,
    })
}

fn subblock_borders(
    reconstructed: &Plane,
    pixels: &[u8; 256],
    (x, y): (usize, usize),
    block: usize,
) -> predict::Borders {
    let (dx, dy) = subblock_offset(block);
    let block_x = x + dx;
    let block_y = y + dy;
    let sample = |column: usize, row: usize| {
        if column >= x && column < x + MACROBLOCK_SIDE && row >= y && row < y + MACROBLOCK_SIDE {
            pixels[(row - y) * MACROBLOCK_SIDE + column - x]
        } else {
            reconstructed.sample(column, row)
        }
    };
    let corner = if block_y == 0 {
        127
    } else if block_x == 0 {
        129
    } else {
        sample(block_x - 1, block_y - 1)
    };
    let mut left = [129; 4];
    if block_x > 0 {
        for (row, value) in left.iter_mut().enumerate() {
            *value = sample(block_x - 1, block_y + row);
        }
    }
    let mut top = [127; 8];
    if block_y > 0 {
        for (column, value) in top[..4].iter_mut().enumerate() {
            *value = sample(block_x + column, block_y - 1);
        }
        // Rightmost subblocks retain the macroblock's original above-right edge.
        let above_right_y = if dx + SUBBLOCK_SIDE == MACROBLOCK_SIDE {
            y.checked_sub(1)
        } else {
            Some(block_y - 1)
        };
        if let Some(row) = above_right_y {
            for (column, value) in top[4..].iter_mut().enumerate() {
                *value = sample(
                    (block_x + 4 + column).min(reconstructed.frame_stride() - 1),
                    row,
                );
            }
        }
    }
    predict::Borders::new(&top, &left, corner)
}

/// The scoring inputs of one subblock candidate.
struct SubblockInputs<'a, S: Simd> {
    quantizer: &'a Quantizer<S>,
    samples: &'a [u8; 16],
    prediction: &'a BlockPrediction,
    mode_cost: u64,
    context: usize,
    method: Quantization,
    previous_levels: Option<&'a [i16; 16]>,
    score_limit: u64,
}

fn analyze_subblock<S: Simd>(inputs: SubblockInputs<'_, S>) -> Option<BlockCandidate> {
    let SubblockInputs {
        quantizer,
        samples,
        prediction,
        mode_cost,
        context,
        method,
        previous_levels,
        score_limit,
    } = inputs;
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
fn score_intra16<S: Simd>(
    samples: &[[u8; 16]; 16],
    prediction: &mut Intra16Prediction,
    quantizer: &Quantizer<S>,
    mut neighbors: MacroblockContexts,
    method: Quantization,
    score_limit: u64,
    fixed_rate: u64,
) -> Option<(Intra16Candidate, u64)> {
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
        let context = neighbors.y(block);
        if matches!(method, Quantization::Nearest) {
            prediction.coefficients[block] =
                residual_coefficients(quantizer.simd, samples, &prediction.blocks[block]);
        }
        let block_levels = quantizer.block(
            &prediction.coefficients[block],
            CoefficientPlane::LumaAc,
            context,
            method,
        );
        let mut coefficients =
            quantize::dequantize_block(&block_levels, factors.y_dc, factors.y_ac);
        coefficients[0] = y2[block];
        let residual = transform::inverse_dct(quantizer.simd, &coefficients);
        let (rebuilt, error) = transform::reconstruct_sse(
            quantizer.simd,
            samples,
            &prediction.blocks[block],
            &residual,
        );
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
        levels[block] = block_levels;
        neighbors.set_y(block, block_levels != [0; 16]);
        store_subblock(&mut pixels, block, &rebuilt);
    }

    Some((
        Intra16Candidate {
            dc: y2_levels,
            ac: levels,
            pixels,
        },
        score,
    ))
}

fn block_sum(samples: &[u8; 16]) -> i32 {
    samples.iter().map(|&sample| i32::from(sample)).sum()
}

fn quantize_chroma_plane<S: Simd>(
    samples: &[u8; 64],
    prediction: &ChromaPrediction<S>,
    quantizer: &Quantizer<S>,
    mut neighbors: MacroblockContexts,
    plane: usize,
    method: Quantization,
) -> ChromaCandidate {
    let matrix = &quantizer.chroma;
    let (candidate_levels, levels) = match method {
        Quantization::Nearest => {
            let levels = matrix.quantize_lanes(quantizer.simd, &prediction.coefficients);
            (transform::block_levels(&levels), levels)
        }
        Quantization::Trellis => {
            // Each plane's context depends on its previous block. Dequantization and
            // reconstruction are batched again once all four levels are available.
            let coefficients = transform::block_coefficients(&prediction.coefficients);
            let mut levels = [[0i16; 16]; 4];
            for block in 0..4 {
                let context = if plane == 0 {
                    neighbors.u(block)
                } else {
                    neighbors.v(block)
                };
                levels[block] = quantizer.block(
                    &coefficients[block],
                    CoefficientPlane::Chroma,
                    context,
                    method,
                );
                let nonzero = levels[block] != [0; 16];
                if plane == 0 {
                    neighbors.set_u(block, nonzero);
                } else {
                    neighbors.set_v(block, nonzero);
                }
            }
            (levels, transform::levels_to_lanes(quantizer.simd, &levels))
        }
    };
    let dequantized = matrix.dequantize_lanes(quantizer.simd, &levels);
    let residual = transform::inverse_dct_lanes(quantizer.simd, &dequantized);
    let (pixels, block_sse) =
        transform::reconstruct_8x8(quantizer.simd, samples, &prediction.pixels, &residual);
    ChromaCandidate {
        levels: candidate_levels,
        pixels,
        sse: block_sse.into_iter().sum(),
    }
}

fn sample_block(plane: &[u8], stride: usize, x: usize, y: usize) -> [u8; 16] {
    let mut block = [0u8; 16];
    for (row, chunk) in block.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let start = (y + row) * stride + x;
        chunk.copy_from_slice(&plane[start..start + 4]);
    }
    block
}

fn subblock_offset(block: usize) -> (usize, usize) {
    (
        block % BLOCKS_PER_ROW * SUBBLOCK_SIDE,
        block / BLOCKS_PER_ROW * SUBBLOCK_SIDE,
    )
}

fn store_subblock(plane: &mut [u8; 256], block: usize, pixels: &[u8; 16]) {
    let (x, y) = subblock_offset(block);
    for (row, pixels) in pixels.as_chunks::<SUBBLOCK_SIDE>().0.iter().enumerate() {
        let start = (y + row) * MACROBLOCK_SIDE + x;
        plane[start..start + SUBBLOCK_SIDE].copy_from_slice(pixels);
    }
}

fn residual_coefficients<S: Simd>(simd: S, samples: &[u8; 16], prediction: &[u8; 16]) -> [i32; 16] {
    let residual =
        std::array::from_fn(|index| i32::from(samples[index]) - i32::from(prediction[index]));
    transform::forward_dct(simd, &residual)
}

fn commit_pixels(plane: &mut Plane, (x, y): (usize, usize), side: usize, pixels: &[u8]) {
    let x = x * side;
    let y = y * side;
    for (row, pixels) in pixels.chunks_exact(side).enumerate() {
        plane.write(y + row, x, pixels);
    }
}
