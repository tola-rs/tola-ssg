//! VP8 trellis quantization minimizing transform distortion plus token rate.

use std::cell::Cell;
use std::sync::Arc;

use crate::TrellisMode;
use crate::quantize::{Factors, RECIPROCAL_SHIFT, reciprocal};
use crate::token::{
    BANDS, BLOCK_COEFFICIENTS, COEFF_BANDS, CONTEXTS, CoefficientPlane, MAGNITUDES, TOKEN_END,
    TOKENS, TokenCosts, ZIGZAG, token_for_magnitude,
};

const CACHED_COEFFICIENTS: usize = 128;
const EDGE_CELLS: usize = CoefficientPlane::ALL.len() * BANDS * CACHED_COEFFICIENTS;

const TOKEN_OF: [u8; MAGNITUDES + 1] = {
    let mut tokens = [0; MAGNITUDES + 1];
    let mut mag = 0;
    while mag < tokens.len() {
        tokens[mag] = token_for_magnitude(mag as u16);
        mag += 1;
    }
    tokens
};

type MagRates = [u32; MAGNITUDES + 1];

pub(crate) const fn restricted_scratch_bytes(strips: u64) -> u64 {
    strips * size_of::<Quantizer>() as u64
        + (size_of::<FrameRates>() + 2 * size_of::<MagRates>()) as u64
}

struct BandRates {
    root: u32,
    tokens: [u32; TOKENS],
}

#[derive(Default)]
struct PosRates {
    end: [u32; CONTEXTS],
    zero: [u32; CONTEXTS],
    one: [u32; CONTEXTS],
    omit_root: u32,
    edge_base: usize,
}

struct PlaneRates {
    pos: [PosRates; BLOCK_COEFFICIENTS + 1],
    bands: [[BandRates; CONTEXTS]; BANDS],
    dc: i32,
    ac: i32,
    dc_recip: u32,
    ac_recip: u32,
    lambda: u64,
}

impl PlaneRates {
    fn new(
        plane: CoefficientPlane,
        dc: i32,
        ac: i32,
        lambda: u64,
        sign_rate: u32,
        costs: &TokenCosts,
    ) -> Self {
        let scale = |rate| (lambda * u64::from(rate)) as u32;
        let bands = std::array::from_fn(|band| {
            std::array::from_fn(|ctx| {
                let (root, tokens) = costs.band(plane, band, ctx);
                BandRates {
                    root: scale(root),
                    tokens: tokens.map(scale),
                }
            })
        });
        let first = plane.first_position();
        let pos = std::array::from_fn(|pos| {
            let Some(&band) = COEFF_BANDS.get(pos) else {
                return PosRates::default();
            };
            let omit_root = if pos == first { 0 } else { bands[band][0].root };
            let mut rates = PosRates {
                omit_root,
                edge_base: (plane.index() * BANDS + band) * CACHED_COEFFICIENTS,
                ..PosRates::default()
            };
            for (ctx, band_rates) in bands[band].iter().enumerate() {
                let root = if ctx == 0 { omit_root } else { 0 };
                rates.end[ctx] = band_rates.tokens[usize::from(TOKEN_END)];
                rates.zero[ctx] = band_rates.tokens[0] - root;
                rates.one[ctx] = band_rates.tokens[1] - root + sign_rate;
            }
            rates
        });
        Self {
            pos,
            bands,
            dc,
            ac,
            dc_recip: reciprocal(dc) as u32,
            ac_recip: reciprocal(ac) as u32,
            lambda,
        }
    }
}

struct FrameRates {
    planes: [PlaneRates; CoefficientPlane::ALL.len()],
    luma_mags: Box<MagRates>,
    chroma_mags: Box<MagRates>,
}

fn mag_rates(costs: &TokenCosts, lambda: u64) -> Box<MagRates> {
    Box::new(std::array::from_fn(|mag| {
        let mag = mag.min(MAGNITUDES - 1) as u16;
        (lambda * u64::from(costs.magnitude_cost(mag))) as u32
    }))
}

#[derive(Clone, Copy)]
struct CtxTwoEdges {
    scores: [u64; 3],
    levels: [u16; 3],
}

impl CtxTwoEdges {
    const EMPTY: Self = Self {
        scores: [u64::MAX; 3],
        levels: [0; 3],
    };
}

#[derive(Clone, Copy)]
struct State {
    score: u64,
    ctx: usize,
}

impl State {
    const DEAD: Self = Self {
        score: u64::MAX,
        ctx: 0,
    };
}

pub(crate) struct Quantizer {
    rates: Arc<FrameRates>,
    edges: Box<[Cell<CtxTwoEdges>]>,
    mode: TrellisMode,
}

impl Quantizer {
    pub(crate) fn new(
        factors: Factors,
        luma_lambda: u64,
        chroma_lambda: u64,
        costs: &TokenCosts,
        mode: TrellisMode,
    ) -> Self {
        let count = if mode == TrellisMode::Exact && (luma_lambda != 0 || chroma_lambda != 0) {
            EDGE_CELLS
        } else {
            0
        };
        let luma_mags = mag_rates(costs, luma_lambda);
        let chroma_mags = mag_rates(costs, chroma_lambda);
        let planes = CoefficientPlane::ALL.map(|plane| {
            let (dc, ac, lambda, sign_rate) = match plane {
                CoefficientPlane::LumaAc | CoefficientPlane::Luma => {
                    (factors.y_dc, factors.y_ac, luma_lambda, luma_mags[1])
                }
                CoefficientPlane::LumaDc => {
                    (factors.y2_dc, factors.y2_ac, luma_lambda, luma_mags[1])
                }
                CoefficientPlane::Chroma => (
                    factors.chroma_dc,
                    factors.chroma_ac,
                    chroma_lambda,
                    chroma_mags[1],
                ),
            };
            PlaneRates::new(plane, dc, ac, lambda, sign_rate, costs)
        });
        let rates = Arc::new(FrameRates {
            planes,
            luma_mags,
            chroma_mags,
        });
        Self {
            rates,
            edges: vec![Cell::new(CtxTwoEdges::EMPTY); count].into_boxed_slice(),
            mode,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn fork(&self) -> Self {
        Self {
            rates: Arc::clone(&self.rates),
            edges: vec![Cell::new(CtxTwoEdges::EMPTY); self.edges.len()].into_boxed_slice(),
            mode: self.mode,
        }
    }

    pub(crate) fn quantize(
        &self,
        coeff: &[i32; 16],
        plane: CoefficientPlane,
        ctx0: usize,
    ) -> [i16; 16] {
        let mags = if matches!(plane, CoefficientPlane::Chroma) {
            &self.rates.chroma_mags
        } else {
            &self.rates.luma_mags
        };
        BlockQuantizer {
            plane,
            rates: &self.rates.planes[plane.index()],
            mags,
            edges: &self.edges,
            mode: self.mode,
        }
        .quantize(coeff, ctx0)
    }
}

struct BlockQuantizer<'a> {
    plane: CoefficientPlane,
    rates: &'a PlaneRates,
    mags: &'a MagRates,
    edges: &'a [Cell<CtxTwoEdges>],
    mode: TrellisMode,
}

impl BlockQuantizer<'_> {
    fn quantize(&self, coeff: &[i32; 16], ctx0: usize) -> [i16; 16] {
        let first = self.plane.first_position();
        if self.rates.lambda == 0 {
            let mut levels = crate::quantize::quantize_block(coeff, self.rates.dc, self.rates.ac);
            if first == 1 {
                levels[0] = 0;
            }
            return levels;
        }
        let w = self.plane.distortion_weight();
        if matches!(self.mode, TrellisMode::Restricted) {
            return self.restricted(coeff, first, ctx0, w);
        }
        // Cheapest nonzero choice adds q * (q - 2|c|) * w at the max |c| within the neutral step.
        let mut zero_nearest = true;
        let mut min_nz_pen = u64::MAX;
        if first == 0 {
            let twice = 2 * coeff[ZIGZAG[0]].unsigned_abs();
            if twice > self.rates.dc as u32 {
                zero_nearest = false;
            } else {
                let q = self.rates.dc as u64;
                min_nz_pen = q * (q - u64::from(twice)) * w;
            }
        }
        let mut max_ac = 0u32;
        for pos in first.max(1)..16 {
            max_ac = max_ac.max(coeff[ZIGZAG[pos]].unsigned_abs());
        }
        let twice = 2 * max_ac;
        if twice > self.rates.ac as u32 {
            zero_nearest = false;
        } else {
            let q = self.rates.ac as u64;
            min_nz_pen = min_nz_pen.min(q * (q - u64::from(twice)) * w);
        }
        if zero_nearest && min_nz_pen >= u64::from(self.rates.pos[first].end[ctx0]) {
            return [0; 16];
        }
        self.exact_dp(coeff, first, ctx0, w)
    }

    fn exact_dp(&self, coeff: &[i32; 16], first: usize, ctx0: usize, w: u64) -> [i16; 16] {
        let mut choices = [[-1i16; CONTEXTS]; BLOCK_COEFFICIENTS];
        let mut tail_err;
        let last = BLOCK_COEFFICIENTS - 1;
        // Position 15 peeled to skip unreachable-state and ctx-0 EOB checks.
        let mut suffix = {
            let pos = last;
            let raster = ZIGZAG[pos];
            let q = self.rates.ac;
            let c = coeff[raster].unsigned_abs();
            let rates = &self.rates.pos[pos];
            let nonunit = self.ctx_two_edges(pos, rates.edge_base, c);
            let err = c as i32;
            tail_err = (err * err) as u64 * w;
            let err = err - q;
            let one_err = (err * err) as u64 * w;
            let omit_root = rates.omit_root;
            let mut prefix = [u64::MAX; CONTEXTS];
            for ctx in 0..CONTEXTS {
                if ctx != 0 {
                    prefix[ctx] = tail_err + u64::from(rates.end[ctx]);
                }
                let score = one_err + u64::from(rates.one[ctx]);
                if score < prefix[ctx] {
                    prefix[ctx] = score;
                    choices[pos][ctx] = 1;
                }
                let score = nonunit.scores[ctx] - if ctx == 0 { u64::from(omit_root) } else { 0 };
                if score < prefix[ctx] {
                    prefix[ctx] = score;
                    choices[pos][ctx] = nonunit.levels[ctx] as i16;
                }
            }
            prefix
        };
        for pos in (first..last).rev() {
            let raster = ZIGZAG[pos];
            let q = self.factor(pos);
            let c = coeff[raster].unsigned_abs();
            let rates = &self.rates.pos[pos];
            let nonunit = self.ctx_two_edges(pos, rates.edge_base, c);
            let err = c as i32;
            let own_err = (err * err) as u64 * w;
            tail_err += own_err;
            let err = err - q;
            let one_err = (err * err) as u64 * w;
            let zero_cont = own_err + suffix[0];
            let one_cont = one_err + suffix[1];
            let omit_root = rates.omit_root;
            let mut prefix = [u64::MAX; 3];
            {
                let zero = zero_cont + u64::from(rates.zero[0]);
                let one = one_cont + u64::from(rates.one[0]);
                let mut best = if pos == first {
                    tail_err + u64::from(rates.end[0])
                } else {
                    u64::MAX
                };
                if zero < best {
                    best = zero;
                    choices[pos][0] = 0;
                }
                if one < best {
                    best = one;
                    choices[pos][0] = 1;
                }
                let score = nonunit.scores[0] - u64::from(omit_root) + suffix[2];
                if score < best {
                    best = score;
                    choices[pos][0] = nonunit.levels[0] as i16;
                }
                prefix[0] = best;
            }
            for ctx in 1..3 {
                let mut best = tail_err + u64::from(rates.end[ctx]);
                let zero = zero_cont + u64::from(rates.zero[ctx]);
                if zero < best {
                    best = zero;
                    choices[pos][ctx] = 0;
                }
                let one = one_cont + u64::from(rates.one[ctx]);
                if one < best {
                    best = one;
                    choices[pos][ctx] = 1;
                }
                let score = nonunit.scores[ctx] + suffix[2];
                if score < best {
                    best = score;
                    choices[pos][ctx] = nonunit.levels[ctx] as i16;
                }
                prefix[ctx] = best;
            }
            suffix = prefix;
        }
        let mut levels = [0; 16];
        let mut mag = choices[first][ctx0];
        for pos in first..BLOCK_COEFFICIENTS {
            if mag < 0 {
                break;
            }
            let raster = ZIGZAG[pos];
            levels[raster] = if coeff[raster] < 0 { -mag } else { mag };
            if pos != last {
                mag = choices[pos + 1][usize::from((mag as u16).min(2))];
            }
        }
        levels
    }

    fn factor(&self, pos: usize) -> i32 {
        if pos == 0 {
            self.rates.dc
        } else {
            self.rates.ac
        }
    }

    /// Neutral level, plus one above it when still below the nearest-quantizer midpoint.
    fn candidates(&self, mag: u32, q: i32, pos: usize) -> ([u16; 2], usize) {
        let q = q as u32;
        let recip = if pos == 0 {
            self.rates.dc_recip
        } else {
            self.rates.ac_recip
        };
        let est = (mag * recip) >> RECIPROCAL_SHIFT;
        let quot = est + u32::from(mag - est * q >= q);
        let rem = mag - quot * q;
        let neutral = quot.min(2047) as u16;
        let nearest = if quot > 2047 {
            2047
        } else {
            quot + u32::from(2 * rem >= q)
        } as u16;
        if neutral < nearest {
            ([neutral, neutral + 1], 2)
        } else {
            ([neutral, neutral], 1)
        }
    }

    fn level_rate(&self, pos: usize, ctx: usize, mag: u16, prev_zero: bool) -> u64 {
        match mag {
            0 => u64::from(self.rates.pos[pos].zero[ctx]),
            1 => u64::from(self.rates.pos[pos].one[ctx]),
            _ => {
                let band = &self.rates.bands[COEFF_BANDS[pos]][ctx];
                let mag = usize::from(mag);
                let tok = usize::from(TOKEN_OF[mag]);
                let root = if prev_zero { band.root } else { 0 };
                u64::from(band.tokens[tok] - root) + u64::from(self.mags[mag])
            }
        }
    }

    fn level_distortion(&self, mag: u32, level: u16, q: i32, w: u64) -> u64 {
        let err = i64::from(mag) - i64::from(level) * i64::from(q);
        (err * err) as u64 * w
    }

    fn restricted(&self, coeff: &[i32; 16], first: usize, ctx0: usize, w: u64) -> [i16; 16] {
        let Some(last) = (first..16).rev().find(|&pos| {
            let mag = coeff[ZIGZAG[pos]].unsigned_abs();
            mag >= (self.factor(pos) as u32).div_ceil(2)
        }) else {
            return [0; 16];
        };
        let mut zero_tail = [0u64; 17];
        for pos in (first..=last).rev() {
            let c = i64::from(coeff[ZIGZAG[pos]]);
            zero_tail[pos] = zero_tail[pos + 1] + (c * c) as u64 * w;
        }
        let mut back_prev = [[0u8; 2]; 16];
        let mut back_level = [[0u16; 2]; 16];
        let mut prev = [State::DEAD; 2];
        let mut cur = prev;
        let mut prev_count = 0;
        let mut best_score = zero_tail[first] + u64::from(self.rates.pos[first].end[ctx0]);
        let mut best_end = u32::MAX;
        for pos in first..=last {
            let raster = ZIGZAG[pos];
            let mag = coeff[raster].unsigned_abs();
            let q = self.factor(pos);
            let (cands, count) = self.candidates(mag, q, pos);
            for (i, &level) in cands.iter().enumerate().take(count) {
                let sse = self.level_distortion(mag, level, q, w);
                let ctx = usize::from(level.min(2));
                let (score, pred) = if pos == first {
                    (self.level_rate(pos, ctx0, level, false) + sse, 0)
                } else {
                    let mut best = (u64::MAX, 0);
                    for (j, st) in prev[..prev_count].iter().enumerate() {
                        let s = st.score + self.level_rate(pos, st.ctx, level, st.ctx == 0) + sse;
                        if s < best.0 {
                            best = (s, j);
                        }
                    }
                    best
                };
                cur[i] = State { score, ctx };
                back_prev[pos][i] = pred as u8;
                back_level[pos][i] = level;
                if ctx != 0 {
                    let eob = zero_tail[pos + 1] + u64::from(self.rates.pos[pos + 1].end[ctx]);
                    if score + eob < best_score {
                        best_score = score + eob;
                        best_end = (pos * 2 + i) as u32;
                    }
                }
            }
            prev = cur;
            prev_count = count;
        }
        let mut levels = [0; 16];
        if best_end != u32::MAX {
            let end = best_end as usize / 2;
            let mut st = best_end as usize & 1;
            for pos in (first..=end).rev() {
                let raster = ZIGZAG[pos];
                let level = back_level[pos][st] as i16;
                levels[raster] = if coeff[raster] < 0 { -level } else { level };
                st = usize::from(back_prev[pos][st]);
            }
        }
        levels
    }

    fn ctx_two_edges(&self, pos: usize, edge_base: usize, c: u32) -> CtxTwoEdges {
        if c < CACHED_COEFFICIENTS as u32 && !self.edges.is_empty() {
            let cell = &self.edges[edge_base + c as usize];
            let cached = cell.get();
            if cached.scores[0] != u64::MAX {
                return cached;
            }
            let edges = self.uncached_ctx_two_edges(pos, c);
            cell.set(edges);
            edges
        } else {
            self.uncached_ctx_two_edges(pos, c)
        }
    }

    fn uncached_ctx_two_edges(&self, pos: usize, c: u32) -> CtxTwoEdges {
        let q = self.factor(pos);
        let w = self.plane.distortion_weight();
        let nearest = ((2 * u64::from(c) + q as u64) / (2 * q as u64)).min(2047) as u16;
        let mut edges = CtxTwoEdges::EMPTY;
        let band = &self.rates.bands[COEFF_BANDS[pos]];
        for (tok, &(lo, hi)) in MAGNITUDE_RANGES.iter().enumerate().skip(2) {
            let closest = nearest.clamp(lo, hi);
            let err = i64::from(c) - i64::from(closest) * i64::from(q);
            let bound = (err * err) as u64 * w + u64::from(self.mags[usize::from(lo)]);
            if bound >= *edges.scores.iter().max().unwrap() {
                continue;
            }
            let mut rates = [0; 3];
            let mut can_improve = false;
            for (ctx, rate) in rates.iter_mut().enumerate() {
                *rate = u64::from(band[ctx].tokens[tok]);
                can_improve |= bound + *rate < edges.scores[ctx];
            }
            if !can_improve {
                continue;
            }
            let (mag, score) = if closest == lo {
                (lo, bound)
            } else {
                category_choice(c, q, w, self.mags, (lo, hi), closest)
            };
            for (ctx, &rate) in rates.iter().enumerate() {
                let s = score + rate;
                if s < edges.scores[ctx] {
                    edges.scores[ctx] = s;
                    edges.levels[ctx] = mag;
                }
            }
        }
        edges
    }
}

const MAGNITUDE_RANGES: [(u16, u16); 11] = [
    (0, 0),
    (1, 1),
    (2, 2),
    (3, 3),
    (4, 4),
    (5, 6),
    (7, 10),
    (11, 18),
    (19, 34),
    (35, 66),
    (67, 2047),
];

fn category_choice(
    c: u32,
    q: i32,
    w: u64,
    mags: &MagRates,
    (lo, hi): (u16, u16),
    nearest: u16,
) -> (u16, u64) {
    let sse = |mag| {
        let err = i64::from(c) - i64::from(mag) * i64::from(q);
        (err * err) as u64 * w
    };
    let rate = |mag| u64::from(mags[usize::from(mag)]);
    let mut best = (nearest, sse(nearest) + rate(nearest));
    // Extra-bit probs exceed 128, so the all-zero offset is cheapest.
    let min_rate = rate(lo);
    for mag in (lo..nearest).rev() {
        let err = sse(mag);
        if err + min_rate >= best.1 {
            break;
        }
        let s = err + rate(mag);
        if s < best.1 {
            best = (mag, s);
        }
    }
    for mag in nearest + 1..=hi {
        let err = sse(mag);
        if err + min_rate >= best.1 {
            break;
        }
        let s = err + rate(mag);
        if s < best.1 {
            best = (mag, s);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantize_block(
        coeff: &[i32; 16],
        dc: i32,
        ac: i32,
        plane: CoefficientPlane,
        ctx0: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> [i16; 16] {
        let mags = mag_rates(costs, lambda);
        let rates = PlaneRates::new(plane, dc, ac, lambda, mags[1], costs);
        BlockQuantizer {
            plane,
            rates: &rates,
            mags: &mags,
            edges: &[],
            mode: TrellisMode::Exact,
        }
        .quantize(coeff, ctx0)
    }

    fn probs() -> [u8; 1056] {
        std::array::from_fn(|i| (17 + i * 73 % 238) as u8)
    }

    #[test]
    fn zero_lambda_nearest() {
        let costs = TokenCosts::new(&probs());
        let coeff = [
            6, -4, 12, -20, 28, 36, -44, 52, 0, 1, -1, 16_380, -16_380, 3, -3, 5,
        ];
        for plane in CoefficientPlane::ALL {
            let mut nearest = crate::quantize::quantize_block(&coeff, 11, 8);
            if plane.first_position() == 1 {
                nearest[0] = 0;
            }
            assert_eq!(quantize_block(&coeff, 11, 8, plane, 0, 0, &costs), nearest);
        }
    }

    fn plane_factors(factors: Factors, plane: CoefficientPlane) -> (i32, i32) {
        match plane {
            CoefficientPlane::LumaAc | CoefficientPlane::Luma => (factors.y_dc, factors.y_ac),
            CoefficientPlane::LumaDc => (factors.y2_dc, factors.y2_ac),
            CoefficientPlane::Chroma => (factors.chroma_dc, factors.chroma_ac),
        }
    }

    fn ref_score(
        coeff: &[i32; 16],
        factors: Factors,
        plane: CoefficientPlane,
        ctx0: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> u64 {
        let first = plane.first_position();
        let w = plane.distortion_weight();
        let (dc, ac) = plane_factors(factors, plane);
        let factor = |pos: usize| if ZIGZAG[pos] == 0 { dc } else { ac };
        let mut prefixes = [u64::MAX; 3];
        prefixes[ctx0] = 0;
        let mut min_score = u64::MAX;
        for pos in first..16 {
            let mut next = [u64::MAX; 3];
            let tail: u64 = ZIGZAG[pos..]
                .iter()
                .map(|&r| {
                    let c = i64::from(coeff[r]);
                    (c * c) as u64 * w
                })
                .sum();
            for (ctx, prefix) in prefixes.into_iter().enumerate() {
                if prefix == u64::MAX {
                    continue;
                }
                let prev_zero = pos != first && ctx == 0;
                if pos == first || ctx != 0 {
                    min_score = min_score
                        .min(prefix + tail + lambda * u64::from(costs.end(plane, pos, ctx)));
                }
                for mag in 0..=2047u16 {
                    let err = i64::from(coeff[ZIGZAG[pos]].unsigned_abs())
                        - i64::from(mag) * i64::from(factor(pos));
                    let score = prefix
                        + (err * err) as u64 * w
                        + lambda * u64::from(costs.coefficient(plane, pos, ctx, prev_zero, mag));
                    if pos == 15 {
                        if mag != 0 {
                            min_score = min_score.min(score);
                        }
                    } else {
                        let next_ctx = usize::from(mag.min(2));
                        next[next_ctx] = next[next_ctx].min(score);
                    }
                }
            }
            prefixes = next;
        }
        min_score
    }

    fn objective(
        coeff: &[i32; 16],
        levels: &[i16; 16],
        factors: Factors,
        plane: CoefficientPlane,
        ctx0: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> u64 {
        let w = plane.distortion_weight();
        let (dc, ac) = plane_factors(factors, plane);
        let sse: u64 = ZIGZAG[plane.first_position()..]
            .iter()
            .map(|&r| {
                let q = if r == 0 { dc } else { ac };
                let err = i64::from(coeff[r] - i32::from(levels[r]) * q);
                (err * err) as u64
            })
            .sum();
        sse * w + lambda * costs.block(levels, plane, ctx0)
    }

    #[test]
    fn exact_trellis_optimal() {
        let factors = Factors {
            y_dc: 11,
            y_ac: 8,
            y2_dc: 11,
            y2_ac: 8,
            chroma_dc: 11,
            chroma_ac: 11,
        };
        let costs = TokenCosts::new(&crate::tables::DEFAULT_COEFF_PROBS);
        for plane in CoefficientPlane::ALL {
            let first = plane.first_position();
            for ctx in 0..3 {
                let mut coeff = [3; 16];
                coeff[ZIGZAG[first]] = -532;
                coeff[ZIGZAG[first + 1]] = 79;
                coeff[ZIGZAG[9]] = -12;
                for lambda in [1024u64, 258_064] {
                    let q = Quantizer::new(factors, lambda, lambda, &costs, TrellisMode::Exact);
                    let levels = q.quantize(&coeff, plane, ctx);
                    assert_eq!(
                        objective(&coeff, &levels, factors, plane, ctx, lambda, &costs),
                        ref_score(&coeff, factors, plane, ctx, lambda, &costs),
                        "trellis {} {ctx} {lambda}",
                        plane.index()
                    );
                }
            }
            for mag in [0, 1, 3] {
                let coeff = [mag; 16];
                let q = Quantizer::new(factors, 16_384, 16_384, &costs, TrellisMode::Exact);
                let levels = q.quantize(&coeff, plane, 0);
                assert_eq!(
                    objective(&coeff, &levels, factors, plane, 0, 16_384, &costs),
                    ref_score(&coeff, factors, plane, 0, 16_384, &costs),
                    "trellis uniform {mag} {}",
                    plane.index()
                );
            }
        }
    }
}
