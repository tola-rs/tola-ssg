//! VP8 intra mode search and macroblock reconstruction.

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
enum QuantMethod {
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
        coeff: &[i32; 16],
        plane: CoefficientPlane,
        ctx: usize,
        quant: QuantMethod,
    ) -> [i16; 16] {
        match quant {
            QuantMethod::Nearest => {
                let mat = match plane {
                    CoefficientPlane::LumaAc | CoefficientPlane::Luma => &self.luma,
                    CoefficientPlane::LumaDc => &self.luma_dc,
                    CoefficientPlane::Chroma => &self.chroma,
                };
                let mut levels = mat.quantize(self.simd, coeff);
                if matches!(plane, CoefficientPlane::LumaAc) {
                    levels[0] = 0;
                }
                levels
            }
            QuantMethod::Trellis => self.trellis.quantize(coeff, plane, ctx),
        }
    }

    fn score(&self, sse: u64, rate: impl FnOnce() -> u64) -> u64 {
        sse * 256
            + if self.lambda == 0 {
                0
            } else {
                self.lambda * rate() / 256
            }
    }
}

// Both orders use exact bounds; only search work changes, not output bytes.
const INTRA4_ACTIVITY_THRESHOLD: u64 = 1200;
const ACTIVITY_GRID: usize = 8;

pub(super) fn prefers_intra4<S: Simd>(simd: S, src: RgbaFrame<'_>) -> bool {
    let stride = MACROBLOCK_SIDE;
    let cols = src.columns();
    let rows = src.rows();
    let step_x = cols.div_ceil(ACTIVITY_GRID);
    let step_y = rows.div_ceil(ACTIVITY_GRID);
    let mut activity = 0u64;
    let mut count = 0u64;
    for mb_y in (0..rows).step_by(step_y) {
        for mb_x in (0..cols).step_by(step_x) {
            let luma = src.luma_block(simd, mb_x, mb_y);
            for blk in 0..BLOCKS_PER_ROW * BLOCKS_PER_ROW {
                let (x, y) = subblock_offset(blk);
                let start = y * stride + x;
                for row in 0..SUBBLOCK_SIDE {
                    let row_start = start + row * stride;
                    for pair in luma[row_start..row_start + SUBBLOCK_SIDE].windows(2) {
                        activity += u64::from(pair[0].abs_diff(pair[1]));
                    }
                }
                for col in 0..SUBBLOCK_SIDE {
                    let mut prev = luma[start + col];
                    for row in 1..SUBBLOCK_SIDE {
                        let val = luma[start + row * stride + col];
                        activity += u64::from(val.abs_diff(prev));
                        prev = val;
                    }
                }
            }
            count += 1;
        }
    }
    activity / count >= INTRA4_ACTIVITY_THRESHOLD
}

struct Intra16Cand {
    dc: [i16; 16],
    ac: [[i16; 16]; 16],
    px: [u8; 256],
}

struct ChromaCand {
    levels: [[i16; 16]; 4],
    px: [u8; 64],
    sse: u64,
}

fn luma_block_samples(src: &YuvRow, col: usize) -> [[u8; 16]; 16] {
    let stride = src.y_stride();
    let x = col * MACROBLOCK_SIDE;
    std::array::from_fn(|blk| {
        let (bx, by) = subblock_offset(blk);
        sample_block(&src.y, stride, x + bx, by)
    })
}

fn chroma_block_samples(src: &YuvRow, col: usize) -> [[u8; 64]; 2] {
    let stride = src.chroma_stride();
    let x = col * CHROMA_SIDE;
    [&src.u, &src.v].map(|plane| plane_block(plane, stride, x))
}

fn plane_block(plane: &[u8], stride: usize, x: usize) -> [u8; 64] {
    let mut blk = [0u8; 64];
    for (row, dst) in blk.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let start = row * stride + x;
        dst.copy_from_slice(&plane[start..start + 8]);
    }
    blk
}

#[derive(Default)]
struct Intra16Pred {
    blocks: [[u8; 16]; 16],
    coeff: [[i32; 16]; 16],
    dc_coeff: [i32; 16],
}

impl Intra16Pred {
    fn predict(&mut self, sums: &[i32; 16], edges: &predict::Edges<16>, mode: u8) {
        let px = predict::luma_block(mode, edges);
        let dc_grid = std::array::from_fn(|blk| {
            let (x, y) = subblock_offset(blk);
            let pred = sample_block(&px, MACROBLOCK_SIDE, x, y);
            let sum = sums[blk] - block_sum(&pred);
            self.blocks[blk] = pred;
            // Forward DCT DC term is (8 * sum + 7) >> 4 == sum >> 1.
            sum >> 1
        });
        self.dc_coeff = transform::forward_wht(&dc_grid);
    }
}

struct ChromaPred<S: Simd> {
    px: [u8; 64],
    coeff: transform::BlockLanes<S>,
}

impl<S: Simd> ChromaPred<S> {
    fn new(simd: S, src: &[u8; 64], edges: &predict::Edges<8>, mode: u8) -> Self {
        let px = predict::chroma_block(mode, edges);
        let coeff = transform::forward_dct_8x8_lanes(simd, src, &px);
        Self { px, coeff }
    }
}

struct ChromaWinner<S: Simd> {
    mode: u8,
    planes: [ChromaCand; 2],
    preds: [ChromaPred<S>; 2],
}

#[derive(Clone, Copy)]
struct BlockPred {
    mode: u8,
    px: [u8; 16],
    coeff: [i32; 16],
}

struct BlockCand {
    pred: BlockPred,
    levels: [i16; 16],
    px: [u8; 16],
    sse: u64,
    score: u64,
    mode_cost: u64,
    rate: u64,
}

struct Intra4Cand {
    levels: [[i16; 16]; 16],
    modes: [u8; 16],
    px: [u8; 256],
    sse: u64,
    rate: u64,
}

pub(super) fn search_macroblock<S: Simd>(
    src: &YuvRow,
    recon: &mut Reconstruction,
    q: &Quantizer<S>,
    ctx: MacroblockContexts,
    bmodes: &vp8::BmodeContexts,
    pos: (usize, usize),
    intra4_first: bool,
) -> DenseMacroblock {
    // Chroma keeps the empty Intra16 luma rate in its rounding, including for Intra4.
    let (empty_y_rate, empty_uv_rate) = if q.lambda == 0 {
        (0, 0)
    } else {
        ctx.empty_rates(&q.costs)
    };
    let col = src.local_column(pos.0);
    let search = MbSearch { q, ctx, pos };
    let luma = search.luma(
        &luma_block_samples(src, col),
        &recon.y,
        bmodes,
        empty_uv_rate,
        intra4_first,
    );
    commit_pixels(&mut recon.y, pos, MACROBLOCK_SIDE, &luma.px);

    let (chroma_mode, [u, v]) = search.chroma(
        &chroma_block_samples(src, col),
        [&recon.u, &recon.v],
        empty_y_rate,
    );
    commit_pixels(&mut recon.u, pos, CHROMA_SIDE, &u.px);
    commit_pixels(&mut recon.v, pos, CHROMA_SIDE, &v.px);
    DenseMacroblock {
        luma: luma.mode,
        y2: luma.y2,
        y: luma.levels,
        chroma_mode,
        u: u.levels,
        v: v.levels,
    }
}

struct MbSearch<'a, S> {
    q: &'a Quantizer<S>,
    ctx: MacroblockContexts,
    pos: (usize, usize),
}

struct LumaChoice {
    mode: Luma,
    y2: [i16; 16],
    px: [u8; 256],
    levels: [[i16; 16]; 16],
}

impl<S: Simd> MbSearch<'_, S> {
    fn luma(
        &self,
        src: &[[u8; 16]; 16],
        recon: &Plane,
        bmodes: &vp8::BmodeContexts,
        empty_uv_rate: u64,
        intra4_first: bool,
    ) -> LumaChoice {
        let (mb_x, mb_y) = self.pos;
        let q = self.q;
        let ctx = self.ctx;
        let edges = predict::Edges::luma(recon, mb_x, mb_y);
        let sums = std::array::from_fn(|blk| block_sum(&src[blk]));
        let mut preds: [Intra16Pred; 2] = Default::default();
        let mut winner = 0;
        preds[winner].predict(&sums, &edges, DC);
        let (mut best_i16, mut best_score) = score_intra16(
            src,
            &mut preds[winner],
            q,
            ctx,
            QuantMethod::Nearest,
            u64::MAX,
            empty_uv_rate + vp8::i16_mode_cost(DC),
        )
        .expect("the first luma mode has no score limit");
        let mut i16_mode = DC;
        for mode in [VERTICAL, HORIZONTAL, TRUE_MOTION] {
            let fixed = empty_uv_rate + vp8::i16_mode_cost(mode);
            if q.score(0, || fixed) >= best_score {
                continue;
            }
            let trial = 1 - winner;
            preds[trial].predict(&sums, &edges, mode);
            if let Some((cand, score)) = score_intra16(
                src,
                &mut preds[trial],
                q,
                ctx,
                QuantMethod::Nearest,
                best_score,
                fixed,
            ) {
                best_i16 = cand;
                best_score = score;
                i16_mode = mode;
                winner = trial;
            }
        }

        let score_i4 = |limit: u64| {
            search_intra4(src, recon, q, ctx, bmodes, self.pos, limit).map(|i4| {
                let score = q.score(i4.sse, || {
                    i4.rate + empty_uv_rate + bmodes.i4_mode_cost(mb_x, &i4.modes)
                });
                (i4, score)
            })
        };
        let mut intra4 = if intra4_first {
            score_i4(best_score)
        } else {
            None
        };
        if q.lambda != 0 {
            // Intra16 wins ties; +1 keeps exact ties when Intra4 ran first.
            let limit = intra4.as_ref().map_or(best_score, |(_, score)| {
                best_score.min(score.saturating_add(1))
            });
            if let Some((refined, score)) = score_intra16(
                src,
                &mut preds[winner],
                q,
                ctx,
                QuantMethod::Trellis,
                limit,
                empty_uv_rate + vp8::i16_mode_cost(i16_mode),
            ) {
                best_i16 = refined;
                best_score = score;
            }
        }
        if !intra4_first {
            intra4 = score_i4(best_score);
        }

        if let Some((i4, score)) = intra4
            && score < best_score
        {
            return LumaChoice {
                mode: Luma::Intra4 { modes: i4.modes },
                y2: [0; 16],
                px: i4.px,
                levels: i4.levels,
            };
        }
        LumaChoice {
            mode: Luma::Intra16 { mode: i16_mode },
            y2: best_i16.dc,
            px: best_i16.px,
            levels: best_i16.ac,
        }
    }

    fn chroma(
        &self,
        src: &[[u8; 64]; 2],
        recon: [&Plane; 2],
        empty_y_rate: u64,
    ) -> (u8, [ChromaCand; 2]) {
        let (x, y) = self.pos;
        let q = self.q;
        let ctx = self.ctx;
        let edges = recon.map(|plane| predict::Edges::chroma(plane, x, y));
        let score_uv = |[u, v]: &[ChromaCand; 2], sse, fixed| {
            q.score(sse, || {
                let mut ctx = ctx;
                fixed + ctx.chroma_cost(&q.costs, &u.levels, &v.levels)
            })
        };
        let mut best = None;
        let mut best_score = u64::MAX;
        for mode in [DC, VERTICAL, HORIZONTAL, TRUE_MOTION] {
            let fixed = empty_y_rate + vp8::chroma_mode_cost(mode);
            if q.score(0, || fixed) >= best_score {
                continue;
            }
            let u_pred = ChromaPred::new(q.simd, &src[0], &edges[0], mode);
            let u = quantize_chroma_plane(&src[0], &u_pred, q, ctx, 0, QuantMethod::Nearest);
            if q.score(u.sse * CHROMA_SAMPLE_AREA, || fixed) >= best_score {
                continue;
            }
            let v_pred = ChromaPred::new(q.simd, &src[1], &edges[1], mode);
            let v = quantize_chroma_plane(&src[1], &v_pred, q, ctx, 1, QuantMethod::Nearest);
            let sse = (u.sse + v.sse) * CHROMA_SAMPLE_AREA;
            if q.score(sse, || fixed) >= best_score {
                continue;
            }
            let planes = [u, v];
            let score = score_uv(&planes, sse, fixed);
            if score < best_score {
                best = Some(ChromaWinner {
                    mode,
                    planes,
                    preds: [u_pred, v_pred],
                });
                best_score = score;
            }
        }
        let ChromaWinner {
            mode,
            mut planes,
            preds,
        } = best.expect("four chroma modes were tried");
        if q.lambda != 0 {
            let refined = std::array::from_fn(|p| {
                quantize_chroma_plane(&src[p], &preds[p], q, ctx, p, QuantMethod::Trellis)
            });
            let sse = (refined[0].sse + refined[1].sse) * CHROMA_SAMPLE_AREA;
            let fixed = empty_y_rate + vp8::chroma_mode_cost(mode);
            if score_uv(&refined, sse, fixed) < best_score {
                planes = refined;
            }
        }
        (mode, planes)
    }
}

fn search_intra4<S: Simd>(
    src: &[[u8; 16]; 16],
    recon: &Plane,
    q: &Quantizer<S>,
    mut ctx: MacroblockContexts,
    bmodes: &vp8::BmodeContexts,
    (mb_x, mb_y): (usize, usize),
    score_limit: u64,
) -> Option<Intra4Cand> {
    let x = mb_x * MACROBLOCK_SIDE;
    let y = mb_y * MACROBLOCK_SIDE;
    let mut levels = [[0i16; 16]; 16];
    let mut modes = [0u8; 16];
    let mut px = [0u8; 256];
    let mut sse = 0u64;
    let mut rate = 0u64;
    let mut score = 0u64;

    for blk in 0..16 {
        let mut best: Option<BlockCand> = None;
        let blk_src = &src[blk];
        let blk_ctx = ctx.y(blk);
        let borders = subblock_borders(recon, &px, (x, y), blk);

        for mode in [B_DC, B_TM, B_VE, B_HE, B_LD, B_RD, B_VR, B_VL, B_HD, B_HU] {
            let limit = best.as_ref().map_or(u64::MAX, |prev| prev.score);
            let mode_cost = bmodes.mode_cost(mb_x, blk, &modes, mode);
            if q.lambda * mode_cost / 256 >= limit {
                continue;
            }
            let pred_px = borders.block(mode);
            let pred = BlockPred {
                mode,
                px: pred_px,
                coeff: residual_coefficients(q.simd, blk_src, &pred_px),
            };
            if let Some(cand) = analyze_subblock(SubblockInputs {
                q,
                src: blk_src,
                pred: &pred,
                mode_cost,
                ctx: blk_ctx,
                quant: QuantMethod::Nearest,
                prev_levels: None,
                limit,
            }) {
                best = Some(cand);
            }
        }

        let mut cand = best.expect("ten subblock modes were tried");
        if q.lambda != 0
            && q.lambda * cand.mode_cost / 256 < cand.score
            && let Some(refined) = analyze_subblock(SubblockInputs {
                q,
                src: blk_src,
                pred: &cand.pred,
                mode_cost: cand.mode_cost,
                ctx: blk_ctx,
                quant: QuantMethod::Trellis,
                prev_levels: Some(&cand.levels),
                limit: cand.score,
            })
        {
            cand = refined;
        }
        score += cand.score;
        if score >= score_limit {
            return None;
        }
        modes[blk] = cand.pred.mode;
        levels[blk] = cand.levels;
        ctx.set_y(blk, cand.levels != [0; 16]);
        sse += cand.sse;
        rate += cand.rate;
        store_subblock(&mut px, blk, &cand.px);
    }

    Some(Intra4Cand {
        levels,
        modes,
        px,
        sse,
        rate,
    })
}

fn subblock_borders(
    recon: &Plane,
    px: &[u8; 256],
    (x, y): (usize, usize),
    blk: usize,
) -> predict::Borders {
    let (dx, dy) = subblock_offset(blk);
    let bx = x + dx;
    let by = y + dy;
    let sample = |col: usize, row: usize| {
        if col >= x && col < x + MACROBLOCK_SIDE && row >= y && row < y + MACROBLOCK_SIDE {
            px[(row - y) * MACROBLOCK_SIDE + col - x]
        } else {
            recon.sample(col, row)
        }
    };
    let corner = if by == 0 {
        127
    } else if bx == 0 {
        129
    } else {
        sample(bx - 1, by - 1)
    };
    let mut left = [129; 4];
    if bx > 0 {
        for (row, v) in left.iter_mut().enumerate() {
            *v = sample(bx - 1, by + row);
        }
    }
    let mut top = [127; 8];
    if by > 0 {
        for (col, v) in top[..4].iter_mut().enumerate() {
            *v = sample(bx + col, by - 1);
        }
        // Rightmost subblocks keep the macroblock's original above-right edge.
        let above_right_y = if dx + SUBBLOCK_SIDE == MACROBLOCK_SIDE {
            y.checked_sub(1)
        } else {
            Some(by - 1)
        };
        if let Some(row) = above_right_y {
            for (col, v) in top[4..].iter_mut().enumerate() {
                *v = sample((bx + 4 + col).min(recon.frame_stride() - 1), row);
            }
        }
    }
    predict::Borders::new(&top, &left, corner)
}

struct SubblockInputs<'a, S: Simd> {
    q: &'a Quantizer<S>,
    src: &'a [u8; 16],
    pred: &'a BlockPred,
    mode_cost: u64,
    ctx: usize,
    quant: QuantMethod,
    prev_levels: Option<&'a [i16; 16]>,
    limit: u64,
}

fn analyze_subblock<S: Simd>(inputs: SubblockInputs<'_, S>) -> Option<BlockCand> {
    let SubblockInputs {
        q,
        src,
        pred,
        mode_cost,
        ctx,
        quant,
        prev_levels,
        limit,
    } = inputs;
    let levels = q.block(&pred.coeff, CoefficientPlane::Luma, ctx, quant);
    if prev_levels.is_some_and(|prev| prev == &levels) {
        return None;
    }
    let coeff = quantize::dequantize_block(&levels, q.factors.y_dc, q.factors.y_ac);
    let res = transform::inverse_dct(q.simd, &coeff);
    let (px, sse) = transform::reconstruct_sse(q.simd, src, &pred.px, &res);
    if sse * 256 + q.lambda * mode_cost / 256 >= limit {
        return None;
    }
    let rate = if q.lambda == 0 {
        0
    } else {
        q.costs.block(&levels, CoefficientPlane::Luma, ctx)
    };
    let score = q.score(sse, || mode_cost + rate);
    (score < limit).then_some(BlockCand {
        pred: *pred,
        levels,
        px,
        sse,
        score,
        mode_cost,
        rate,
    })
}

fn score_intra16<S: Simd>(
    src: &[[u8; 16]; 16],
    pred: &mut Intra16Pred,
    q: &Quantizer<S>,
    mut ctx: MacroblockContexts,
    quant: QuantMethod,
    limit: u64,
    fixed_rate: u64,
) -> Option<(Intra16Cand, u64)> {
    let factors = q.factors;

    let y2_levels = q.block(&pred.dc_coeff, CoefficientPlane::LumaDc, ctx.y2(), quant);
    let y2 = transform::inverse_wht(&quantize::dequantize_block(
        &y2_levels,
        factors.y2_dc,
        factors.y2_ac,
    ));

    let mut rate = if q.lambda == 0 {
        0
    } else {
        q.costs
            .block(&y2_levels, CoefficientPlane::LumaDc, ctx.y2())
    };
    let mut levels = [[0i16; 16]; 16];
    let mut px = [0u8; 256];
    let mut sse = 0u64;
    let mut score = 0;
    for (blk, blk_src) in src.iter().enumerate() {
        let blk_ctx = ctx.y(blk);
        if matches!(quant, QuantMethod::Nearest) {
            pred.coeff[blk] = residual_coefficients(q.simd, blk_src, &pred.blocks[blk]);
        }
        let blk_levels = q.block(&pred.coeff[blk], CoefficientPlane::LumaAc, blk_ctx, quant);
        let mut coeff = quantize::dequantize_block(&blk_levels, factors.y_dc, factors.y_ac);
        coeff[0] = y2[blk];
        let res = transform::inverse_dct(q.simd, &coeff);
        let (rebuilt, err) = transform::reconstruct_sse(q.simd, blk_src, &pred.blocks[blk], &res);
        sse += err;
        if q.lambda != 0 {
            rate += q
                .costs
                .block(&blk_levels, CoefficientPlane::LumaAc, blk_ctx);
        }
        score = sse * 256 + q.lambda * (rate + fixed_rate) / 256;
        if score >= limit {
            return None;
        }
        levels[blk] = blk_levels;
        ctx.set_y(blk, blk_levels != [0; 16]);
        store_subblock(&mut px, blk, &rebuilt);
    }

    Some((
        Intra16Cand {
            dc: y2_levels,
            ac: levels,
            px,
        },
        score,
    ))
}

fn block_sum(src: &[u8; 16]) -> i32 {
    src.iter().map(|&px| i32::from(px)).sum()
}

fn quantize_chroma_plane<S: Simd>(
    src: &[u8; 64],
    pred: &ChromaPred<S>,
    q: &Quantizer<S>,
    mut ctx: MacroblockContexts,
    plane: usize,
    quant: QuantMethod,
) -> ChromaCand {
    let mat = &q.chroma;
    let (cand_levels, levels) = match quant {
        QuantMethod::Nearest => {
            let levels = mat.quantize_lanes(q.simd, &pred.coeff);
            (transform::block_levels(&levels), levels)
        }
        QuantMethod::Trellis => {
            let coeff = transform::block_coefficients(&pred.coeff);
            let mut levels = [[0i16; 16]; 4];
            for blk in 0..4 {
                let blk_ctx = if plane == 0 { ctx.u(blk) } else { ctx.v(blk) };
                levels[blk] = q.block(&coeff[blk], CoefficientPlane::Chroma, blk_ctx, quant);
                let nz = levels[blk] != [0; 16];
                if plane == 0 {
                    ctx.set_u(blk, nz);
                } else {
                    ctx.set_v(blk, nz);
                }
            }
            (levels, transform::levels_to_lanes(q.simd, &levels))
        }
    };
    let dequant = mat.dequantize_lanes(q.simd, &levels);
    let res = transform::inverse_dct_lanes(q.simd, &dequant);
    let (px, blk_sse) = transform::reconstruct_8x8(q.simd, src, &pred.px, &res);
    ChromaCand {
        levels: cand_levels,
        px,
        sse: blk_sse.into_iter().sum(),
    }
}

fn sample_block(plane: &[u8], stride: usize, x: usize, y: usize) -> [u8; 16] {
    let mut blk = [0u8; 16];
    for (row, dst) in blk.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let start = (y + row) * stride + x;
        dst.copy_from_slice(&plane[start..start + 4]);
    }
    blk
}

fn subblock_offset(blk: usize) -> (usize, usize) {
    (
        blk % BLOCKS_PER_ROW * SUBBLOCK_SIDE,
        blk / BLOCKS_PER_ROW * SUBBLOCK_SIDE,
    )
}

fn store_subblock(plane: &mut [u8; 256], blk: usize, px: &[u8; 16]) {
    let (x, y) = subblock_offset(blk);
    for (row, src) in px.as_chunks::<SUBBLOCK_SIDE>().0.iter().enumerate() {
        let start = (y + row) * MACROBLOCK_SIDE + x;
        plane[start..start + SUBBLOCK_SIDE].copy_from_slice(src);
    }
}

fn residual_coefficients<S: Simd>(simd: S, src: &[u8; 16], pred: &[u8; 16]) -> [i32; 16] {
    let res = std::array::from_fn(|i| i32::from(src[i]) - i32::from(pred[i]));
    transform::forward_dct(simd, &res)
}

fn commit_pixels(plane: &mut Plane, (x, y): (usize, usize), side: usize, px: &[u8]) {
    let x = x * side;
    let y = y * side;
    for (row, src) in px.chunks_exact(side).enumerate() {
        plane.write(y + row, x, src);
    }
}
