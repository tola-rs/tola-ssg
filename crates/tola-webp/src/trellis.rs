//! VP8 coefficient quantization minimizing transform distortion plus token rate.

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
    let mut magnitude = 0;
    while magnitude < tokens.len() {
        tokens[magnitude] = token_for_magnitude(magnitude as u16);
        magnitude += 1;
    }
    tokens
};

type MagnitudeRates = [u32; MAGNITUDES + 1];

pub(crate) const fn restricted_scratch_bytes(strips: u64) -> u64 {
    strips * size_of::<Quantizer>() as u64
        + (size_of::<FrameRates>() + 2 * size_of::<MagnitudeRates>()) as u64
}

/// The cost of each branch path from one band and context, scaled by lambda.
struct BandRates {
    root: u32,
    tokens: [u32; TOKENS],
}

/// Small levels and the EOB token are read directly by both searches.
#[derive(Default)]
struct PositionRates {
    end: [u32; CONTEXTS],
    zero: [u32; CONTEXTS],
    one: [u32; CONTEXTS],
    omitted_root: u32,
    edge_base: usize,
}

struct PlaneRates {
    positions: [PositionRates; BLOCK_COEFFICIENTS + 1],
    bands: [[BandRates; CONTEXTS]; BANDS],
    dc: i32,
    ac: i32,
    dc_reciprocal: u32,
    ac_reciprocal: u32,
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
            std::array::from_fn(|context| {
                let (root, tokens) = costs.band(plane, band, context);
                BandRates {
                    root: scale(root),
                    tokens: tokens.map(scale),
                }
            })
        });
        let first = plane.first_position();
        let positions = std::array::from_fn(|position| {
            let Some(&band) = COEFF_BANDS.get(position) else {
                // Ending after the final coefficient emits no EOB token.
                return PositionRates::default();
            };
            let omitted_root = if position == first {
                0
            } else {
                bands[band][0].root
            };
            let mut rates = PositionRates {
                omitted_root,
                // Band zero is DC; every other band shares the plane's AC factor.
                edge_base: (plane.index() * BANDS + band) * CACHED_COEFFICIENTS,
                ..PositionRates::default()
            };
            for (context, band_rates) in bands[band].iter().enumerate() {
                let root = if context == 0 { omitted_root } else { 0 };
                rates.end[context] = band_rates.tokens[usize::from(TOKEN_END)];
                rates.zero[context] = band_rates.tokens[0] - root;
                rates.one[context] = band_rates.tokens[1] - root + sign_rate;
            }
            rates
        });
        Self {
            positions,
            bands,
            dc,
            ac,
            dc_reciprocal: reciprocal(dc) as u32,
            ac_reciprocal: reciprocal(ac) as u32,
            lambda,
        }
    }
}

struct FrameRates {
    planes: [PlaneRates; CoefficientPlane::ALL.len()],
    luma_magnitudes: Box<MagnitudeRates>,
    chroma_magnitudes: Box<MagnitudeRates>,
}

fn magnitude_rates(costs: &TokenCosts, lambda: u64) -> Box<MagnitudeRates> {
    Box::new(std::array::from_fn(|magnitude| {
        let magnitude = magnitude.min(MAGNITUDES - 1) as u16;
        (lambda * u64::from(costs.magnitude_cost(magnitude))) as u32
    }))
}

#[derive(Clone, Copy)]
struct ContextTwoEdges {
    scores: [u64; 3],
    levels: [u16; 3],
}

impl ContextTwoEdges {
    const EMPTY: Self = Self {
        scores: [u64::MAX; 3],
        levels: [0; 3],
    };
}

#[derive(Clone, Copy)]
struct State {
    score: u64,
    context: usize,
}

impl State {
    const DEAD: Self = Self {
        score: u64::MAX,
        context: 0,
    };
}

pub(crate) struct Quantizer {
    rates: Arc<FrameRates>,
    edges: Box<[Cell<ContextTwoEdges>]>,
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
        // Exact strips share frame rates; each strip caches up to 128 KiB of edges.
        let count = if mode == TrellisMode::Exact && (luma_lambda != 0 || chroma_lambda != 0) {
            EDGE_CELLS
        } else {
            0
        };
        let luma_magnitudes = magnitude_rates(costs, luma_lambda);
        let chroma_magnitudes = magnitude_rates(costs, chroma_lambda);
        let planes = CoefficientPlane::ALL.map(|plane| {
            let (dc, ac, lambda, sign_rate) = match plane {
                CoefficientPlane::LumaAc | CoefficientPlane::Luma => {
                    (factors.y_dc, factors.y_ac, luma_lambda, luma_magnitudes[1])
                }
                CoefficientPlane::LumaDc => (
                    factors.y2_dc,
                    factors.y2_ac,
                    luma_lambda,
                    luma_magnitudes[1],
                ),
                CoefficientPlane::Chroma => (
                    factors.chroma_dc,
                    factors.chroma_ac,
                    chroma_lambda,
                    chroma_magnitudes[1],
                ),
            };
            PlaneRates::new(plane, dc, ac, lambda, sign_rate, costs)
        });
        let rates = Arc::new(FrameRates {
            planes,
            luma_magnitudes,
            chroma_magnitudes,
        });
        Self {
            rates,
            edges: vec![Cell::new(ContextTwoEdges::EMPTY); count].into_boxed_slice(),
            mode,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn fork(&self) -> Self {
        Self {
            rates: Arc::clone(&self.rates),
            edges: vec![Cell::new(ContextTwoEdges::EMPTY); self.edges.len()].into_boxed_slice(),
            mode: self.mode,
        }
    }

    pub(crate) fn quantize(
        &self,
        coefficients: &[i32; 16],
        plane: CoefficientPlane,
        first_context: usize,
    ) -> [i16; 16] {
        let magnitudes = if matches!(plane, CoefficientPlane::Chroma) {
            &self.rates.chroma_magnitudes
        } else {
            &self.rates.luma_magnitudes
        };
        BlockQuantizer {
            plane,
            rates: &self.rates.planes[plane.index()],
            magnitudes,
            edges: &self.edges,
            mode: self.mode,
        }
        .quantize(coefficients, first_context)
    }
}

struct BlockQuantizer<'a> {
    plane: CoefficientPlane,
    rates: &'a PlaneRates,
    magnitudes: &'a MagnitudeRates,
    edges: &'a [Cell<ContextTwoEdges>],
    mode: TrellisMode,
}

impl BlockQuantizer<'_> {
    fn quantize(&self, coefficients: &[i32; 16], first_context: usize) -> [i16; 16] {
        let first = self.plane.first_position();
        if self.rates.lambda == 0 {
            let mut levels =
                crate::quantize::quantize_block(coefficients, self.rates.dc, self.rates.ac);
            if first == 1 {
                levels[0] = 0;
            }
            return levels;
        }
        let weight = self.plane.distortion_weight();
        if matches!(self.mode, TrellisMode::Restricted) {
            return self.restricted(coefficients, first, first_context, weight);
        }
        // The cheapest non-zero selection adds `factor * (factor - 2|c|) * weight` at the
        // coefficient with the largest magnitude still inside the neutral step. That is a
        // maximum over the block, so the reverse scan collapses to one pass over the maximum.
        let mut zero_is_nearest = true;
        let mut minimum_nonzero_penalty = u64::MAX;
        if first == 0 {
            let coefficient = coefficients[ZIGZAG[0]].unsigned_abs();
            let twice = 2 * coefficient;
            if twice > self.rates.dc as u32 {
                zero_is_nearest = false;
            } else {
                let factor = self.rates.dc as u64;
                minimum_nonzero_penalty = factor * (factor - u64::from(twice)) * weight;
            }
        }
        let mut maximum = 0u32;
        for position in first.max(1)..16 {
            maximum = maximum.max(coefficients[ZIGZAG[position]].unsigned_abs());
        }
        let twice = 2 * maximum;
        if twice > self.rates.ac as u32 {
            zero_is_nearest = false;
        } else {
            let factor = self.rates.ac as u64;
            minimum_nonzero_penalty =
                minimum_nonzero_penalty.min(factor * (factor - u64::from(twice)) * weight);
        }
        // Any nonempty path adds at least this distortion, even if every token were free.
        if zero_is_nearest
            && minimum_nonzero_penalty >= u64::from(self.rates.positions[first].end[first_context])
        {
            return [0; 16];
        }
        self.exact_dp(coefficients, first, first_context, weight)
    }

    fn exact_dp(
        &self,
        coefficients: &[i32; 16],
        first: usize,
        first_context: usize,
        weight: u64,
    ) -> [i16; 16] {
        let mut choices = [[-1i16; CONTEXTS]; BLOCK_COEFFICIENTS];
        let mut tail_error;
        let last = BLOCK_COEFFICIENTS - 1;
        // Position 15 has no suffix states yet; peel it so later positions can skip the
        // unreachable-state tests and the context-zero EOB test.
        let mut suffix = {
            let position = last;
            let raster = ZIGZAG[position];
            let factor = self.rates.ac;
            let coefficient = coefficients[raster].unsigned_abs();
            let rates = &self.rates.positions[position];
            let nonunit = self.context_two_edges(position, rates.edge_base, coefficient);
            let error = coefficient as i32;
            tail_error = (error * error) as u64 * weight;
            let error = error - factor;
            let one_error = (error * error) as u64 * weight;
            let omitted_root = rates.omitted_root;
            let mut prefix = [u64::MAX; CONTEXTS];
            for context in 0..CONTEXTS {
                if context != 0 {
                    prefix[context] = tail_error + u64::from(rates.end[context]);
                }
                let score = one_error + u64::from(rates.one[context]);
                if score < prefix[context] {
                    prefix[context] = score;
                    choices[position][context] = 1;
                }
                let score = nonunit.scores[context]
                    - if context == 0 {
                        u64::from(omitted_root)
                    } else {
                        0
                    };
                if score < prefix[context] {
                    prefix[context] = score;
                    choices[position][context] = nonunit.levels[context] as i16;
                }
            }
            prefix
        };
        for position in (first..last).rev() {
            let raster = ZIGZAG[position];
            let factor = self.factor(position);
            let coefficient = coefficients[raster].unsigned_abs();
            let rates = &self.rates.positions[position];
            let nonunit = self.context_two_edges(position, rates.edge_base, coefficient);
            let error = coefficient as i32;
            let own_error = (error * error) as u64 * weight;
            tail_error += own_error;
            let error = error - factor;
            let one_error = (error * error) as u64 * weight;
            let zero_continuation = own_error + suffix[0];
            let one_continuation = one_error + suffix[1];
            let omitted_root = rates.omitted_root;
            let mut prefix = [u64::MAX; 3];
            {
                let zero = zero_continuation + u64::from(rates.zero[0]);
                let one = one_continuation + u64::from(rates.one[0]);
                let mut best = if position == first {
                    tail_error + u64::from(rates.end[0])
                } else {
                    u64::MAX
                };
                if zero < best {
                    best = zero;
                    choices[position][0] = 0;
                }
                if one < best {
                    best = one;
                    choices[position][0] = 1;
                }
                let score = nonunit.scores[0] - u64::from(omitted_root) + suffix[2];
                if score < best {
                    best = score;
                    choices[position][0] = nonunit.levels[0] as i16;
                }
                prefix[0] = best;
            }
            for context in 1..3 {
                let mut best = tail_error + u64::from(rates.end[context]);
                let zero = zero_continuation + u64::from(rates.zero[context]);
                if zero < best {
                    best = zero;
                    choices[position][context] = 0;
                }
                let one = one_continuation + u64::from(rates.one[context]);
                if one < best {
                    best = one;
                    choices[position][context] = 1;
                }
                let score = nonunit.scores[context] + suffix[2];
                if score < best {
                    best = score;
                    choices[position][context] = nonunit.levels[context] as i16;
                }
                prefix[context] = best;
            }
            suffix = prefix;
        }
        let mut levels = [0; 16];
        let mut magnitude = choices[first][first_context];
        for position in first..BLOCK_COEFFICIENTS {
            if magnitude < 0 {
                break;
            }
            let raster = ZIGZAG[position];
            levels[raster] = if coefficients[raster] < 0 {
                -magnitude
            } else {
                magnitude
            };
            if position != last {
                magnitude = choices[position + 1][usize::from((magnitude as u16).min(2))];
            }
        }
        levels
    }

    fn factor(&self, position: usize) -> i32 {
        if position == 0 {
            self.rates.dc
        } else {
            self.rates.ac
        }
    }

    /// libwebp-style candidate set: the neutral level, plus one level above it only while
    /// that level is still within the half-biased nearest quantizer.
    fn candidates(&self, magnitude: u32, factor: i32, position: usize) -> ([u16; 2], usize) {
        let factor = factor as u32;
        let reciprocal = if position == 0 {
            self.rates.dc_reciprocal
        } else {
            self.rates.ac_reciprocal
        };
        // As in the SIMD quantizer, a VP8 transform magnitude fits below the reciprocal scale.
        // The estimate can be one below the exact quotient, so correct it from the remainder.
        let estimate = (magnitude * reciprocal) >> RECIPROCAL_SHIFT;
        let quotient = estimate + u32::from(magnitude - estimate * factor >= factor);
        let remainder = magnitude - quotient * factor;
        let neutral = quotient.min(2047) as u16;
        let nearest = if quotient > 2047 {
            2047
        } else {
            quotient + u32::from(2 * remainder >= factor)
        } as u16;
        if neutral < nearest {
            ([neutral, neutral + 1], 2)
        } else {
            ([neutral, neutral], 1)
        }
    }

    fn level_rate(
        &self,
        position: usize,
        context: usize,
        magnitude: u16,
        previous_was_zero: bool,
    ) -> u64 {
        match magnitude {
            0 => u64::from(self.rates.positions[position].zero[context]),
            1 => u64::from(self.rates.positions[position].one[context]),
            _ => {
                let band = &self.rates.bands[COEFF_BANDS[position]][context];
                let magnitude = usize::from(magnitude);
                let token = usize::from(TOKEN_OF[magnitude]);
                let root = if previous_was_zero { band.root } else { 0 };
                u64::from(band.tokens[token] - root) + u64::from(self.magnitudes[magnitude])
            }
        }
    }

    fn level_distortion(&self, magnitude: u32, level: u16, factor: i32, weight: u64) -> u64 {
        let error = i64::from(magnitude) - i64::from(level) * i64::from(factor);
        (error * error) as u64 * weight
    }

    /// Two-candidate forward DP with two live states per position, truncated at `last`.
    fn restricted(
        &self,
        coefficients: &[i32; 16],
        first: usize,
        first_context: usize,
        weight: u64,
    ) -> [i16; 16] {
        let Some(last) = (first..16).rev().find(|&position| {
            let magnitude = coefficients[ZIGZAG[position]].unsigned_abs();
            magnitude >= (self.factor(position) as u32).div_ceil(2)
        }) else {
            return [0; 16];
        };
        // All candidates quantize the positions after last to zero. Their distortion is
        // common to every candidate, so the comparison starts with a zero tail there.
        let mut zero_tail = [0u64; 17];
        for position in (first..=last).rev() {
            let coefficient = i64::from(coefficients[ZIGZAG[position]]);
            zero_tail[position] =
                zero_tail[position + 1] + (coefficient * coefficient) as u64 * weight;
        }
        let mut back_prev = [[0u8; 2]; 16];
        let mut back_level = [[0u16; 2]; 16];
        let mut previous = [State::DEAD; 2];
        let mut current = previous;
        let mut previous_count = 0;
        let mut best_score =
            zero_tail[first] + u64::from(self.rates.positions[first].end[first_context]);
        let mut best_end = u32::MAX;
        for position in first..=last {
            let raster = ZIGZAG[position];
            let magnitude = coefficients[raster].unsigned_abs();
            let factor = self.factor(position);
            let (candidates, count) = self.candidates(magnitude, factor, position);
            for (index, &level) in candidates.iter().enumerate().take(count) {
                let distortion = self.level_distortion(magnitude, level, factor, weight);
                let context = usize::from(level.min(2));
                let (score, predecessor) = if position == first {
                    let rate = self.level_rate(position, first_context, level, false);
                    (rate + distortion, 0)
                } else {
                    let mut best = (u64::MAX, 0);
                    for (index, state) in previous[..previous_count].iter().enumerate() {
                        let score = state.score
                            + self.level_rate(position, state.context, level, state.context == 0)
                            + distortion;
                        if score < best.0 {
                            best = (score, index);
                        }
                    }
                    best
                };
                current[index] = State { score, context };
                back_prev[position][index] = predecessor as u8;
                back_level[position][index] = level;
                if context != 0 {
                    let eob = zero_tail[position + 1]
                        + u64::from(self.rates.positions[position + 1].end[context]);
                    if score + eob < best_score {
                        best_score = score + eob;
                        best_end = (position * 2 + index) as u32;
                    }
                }
            }
            previous = current;
            previous_count = count;
        }
        let mut levels = [0; 16];
        if best_end != u32::MAX {
            let end = best_end as usize / 2;
            let mut state = best_end as usize & 1;
            for position in (first..=end).rev() {
                let raster = ZIGZAG[position];
                let level = back_level[position][state] as i16;
                levels[raster] = if coefficients[raster] < 0 {
                    -level
                } else {
                    level
                };
                state = usize::from(back_prev[position][state]);
            }
        }
        levels
    }

    fn context_two_edges(
        &self,
        position: usize,
        edge_base: usize,
        coefficient: u32,
    ) -> ContextTwoEdges {
        if coefficient < CACHED_COEFFICIENTS as u32 && !self.edges.is_empty() {
            let cell = &self.edges[edge_base + coefficient as usize];
            let cached = cell.get();
            if cached.scores[0] != u64::MAX {
                return cached;
            }
            let edges = self.uncached_context_two_edges(position, coefficient);
            cell.set(edges);
            edges
        } else {
            self.uncached_context_two_edges(position, coefficient)
        }
    }

    fn uncached_context_two_edges(&self, position: usize, coefficient: u32) -> ContextTwoEdges {
        let factor = self.factor(position);
        let weight = self.plane.distortion_weight();
        let nearest =
            ((2 * u64::from(coefficient) + factor as u64) / (2 * factor as u64)).min(2047) as u16;
        let mut edges = ContextTwoEdges::EMPTY;
        let band = &self.rates.bands[COEFF_BANDS[position]];
        for (token, &(lower, upper)) in MAGNITUDE_RANGES.iter().enumerate().skip(2) {
            let closest = nearest.clamp(lower, upper);
            let error = i64::from(coefficient) - i64::from(closest) * i64::from(factor);
            let bound =
                (error * error) as u64 * weight + u64::from(self.magnitudes[usize::from(lower)]);
            if bound >= *edges.scores.iter().max().unwrap() {
                continue;
            }
            let mut rates = [0; 3];
            let mut can_improve = false;
            for (context, rate) in rates.iter_mut().enumerate() {
                *rate = u64::from(band[context].tokens[token]);
                can_improve |= bound + *rate < edges.scores[context];
            }
            if !can_improve {
                continue;
            }
            let (magnitude, score) = if closest == lower {
                (lower, bound)
            } else {
                category_choice(
                    coefficient,
                    factor,
                    weight,
                    self.magnitudes,
                    (lower, upper),
                    closest,
                )
            };
            for (context, &rate) in rates.iter().enumerate() {
                let score = score + rate;
                if score < edges.scores[context] {
                    edges.scores[context] = score;
                    edges.levels[context] = magnitude;
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
    coefficient: u32,
    factor: i32,
    weight: u64,
    magnitudes: &MagnitudeRates,
    (lower, upper): (u16, u16),
    nearest: u16,
) -> (u16, u64) {
    let distortion = |magnitude| {
        let error = i64::from(coefficient) - i64::from(magnitude) * i64::from(factor);
        (error * error) as u64 * weight
    };
    let rate = |magnitude| u64::from(magnitudes[usize::from(magnitude)]);
    let mut best = (nearest, distortion(nearest) + rate(nearest));
    // Every category extra-bit probability exceeds 128, so its all-zero offset is cheapest.
    let minimum_rate = rate(lower);
    for magnitude in (lower..nearest).rev() {
        let error = distortion(magnitude);
        if error + minimum_rate >= best.1 {
            break;
        }
        let score = error + rate(magnitude);
        if score < best.1 {
            best = (magnitude, score);
        }
    }
    for magnitude in nearest + 1..=upper {
        let error = distortion(magnitude);
        if error + minimum_rate >= best.1 {
            break;
        }
        let score = error + rate(magnitude);
        if score < best.1 {
            best = (magnitude, score);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantize_block(
        coefficients: &[i32; 16],
        dc: i32,
        ac: i32,
        plane: CoefficientPlane,
        first_context: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> [i16; 16] {
        let magnitudes = magnitude_rates(costs, lambda);
        let rates = PlaneRates::new(plane, dc, ac, lambda, magnitudes[1], costs);
        BlockQuantizer {
            plane,
            rates: &rates,
            magnitudes: &magnitudes,
            edges: &[],
            mode: TrellisMode::Exact,
        }
        .quantize(coefficients, first_context)
    }

    fn probabilities() -> [u8; 1056] {
        std::array::from_fn(|index| (17 + index * 73 % 238) as u8)
    }

    #[test]
    fn zero_lambda_rounds_to_nearest() {
        let costs = TokenCosts::new(&probabilities());
        let coefficients = [
            6, -4, 12, -20, 28, 36, -44, 52, 0, 1, -1, 16_380, -16_380, 3, -3, 5,
        ];
        for plane in [
            CoefficientPlane::LumaAc,
            CoefficientPlane::LumaDc,
            CoefficientPlane::Chroma,
            CoefficientPlane::Luma,
        ] {
            let mut nearest = crate::quantize::quantize_block(&coefficients, 11, 8);
            if plane.first_position() == 1 {
                nearest[0] = 0;
            }
            assert_eq!(
                quantize_block(&coefficients, 11, 8, plane, 0, 0, &costs),
                nearest
            );
        }
    }

    /// The quantizer's factor pair for one plane; scoring must mirror it.
    fn plane_factors(factors: Factors, plane: CoefficientPlane) -> (i32, i32) {
        match plane {
            CoefficientPlane::LumaAc | CoefficientPlane::Luma => (factors.y_dc, factors.y_ac),
            CoefficientPlane::LumaDc => (factors.y2_dc, factors.y2_ac),
            CoefficientPlane::Chroma => (factors.chroma_dc, factors.chroma_ac),
        }
    }

    fn reference_score(
        coefficients: &[i32; 16],
        factors: Factors,
        plane: CoefficientPlane,
        first_context: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> u64 {
        let first = plane.first_position();
        let weight = plane.distortion_weight();
        let (dc, ac) = plane_factors(factors, plane);
        let factor = |position: usize| if ZIGZAG[position] == 0 { dc } else { ac };
        let mut prefixes = [u64::MAX; 3];
        prefixes[first_context] = 0;
        let mut minimum = u64::MAX;
        for position in first..16 {
            let mut next = [u64::MAX; 3];
            let tail: u64 = ZIGZAG[position..]
                .iter()
                .map(|raster| {
                    let coefficient = i64::from(coefficients[*raster]);
                    (coefficient * coefficient) as u64 * weight
                })
                .sum();
            for (context, prefix) in prefixes.into_iter().enumerate() {
                if prefix == u64::MAX {
                    continue;
                }
                let previous_was_zero = position != first && context == 0;
                if position == first || context != 0 {
                    minimum = minimum.min(
                        prefix + tail + lambda * u64::from(costs.end(plane, position, context)),
                    );
                }
                for magnitude in 0..=2047u16 {
                    let error = i64::from(coefficients[ZIGZAG[position]].unsigned_abs())
                        - i64::from(magnitude) * i64::from(factor(position));
                    let score = prefix
                        + (error * error) as u64 * weight
                        + lambda
                            * u64::from(costs.coefficient(
                                plane,
                                position,
                                context,
                                previous_was_zero,
                                magnitude,
                            ));
                    if position == 15 {
                        if magnitude != 0 {
                            minimum = minimum.min(score);
                        }
                    } else {
                        let next_context = usize::from(magnitude.min(2));
                        next[next_context] = next[next_context].min(score);
                    }
                }
            }
            prefixes = next;
        }
        minimum
    }

    fn objective(
        coefficients: &[i32; 16],
        levels: &[i16; 16],
        factors: Factors,
        plane: CoefficientPlane,
        first_context: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> u64 {
        let weight = plane.distortion_weight();
        let (dc, ac) = plane_factors(factors, plane);
        let distortion: u64 = ZIGZAG[plane.first_position()..]
            .iter()
            .map(|raster| {
                let factor = if *raster == 0 { dc } else { ac };
                let error = i64::from(coefficients[*raster] - i32::from(levels[*raster]) * factor);
                (error * error) as u64
            })
            .sum();
        distortion * weight + lambda * costs.block(levels, plane, first_context)
    }

    /// Every accelerated search returns the optimum the exhaustive search finds.
    #[test]
    fn searches_find_the_exhaustive_optimum() {
        let factors = Factors {
            y_dc: 11,
            y_ac: 8,
            y2_dc: 11,
            y2_ac: 8,
            chroma_dc: 11,
            chroma_ac: 11,
        };
        let costs = TokenCosts::new(&crate::tables::DEFAULT_COEFF_PROBS);
        for plane in [
            CoefficientPlane::LumaAc,
            CoefficientPlane::LumaDc,
            CoefficientPlane::Chroma,
            CoefficientPlane::Luma,
        ] {
            let first = plane.first_position();
            for context in 0..3 {
                let mut coefficients = [3; 16];
                coefficients[ZIGZAG[first]] = -532;
                coefficients[ZIGZAG[first + 1]] = 79;
                coefficients[ZIGZAG[9]] = -12;
                for lambda in [1024u64, 258_064] {
                    let model = Quantizer::new(factors, lambda, lambda, &costs, TrellisMode::Exact);
                    let levels = model.quantize(&coefficients, plane, context);
                    assert_eq!(
                        objective(
                            &coefficients,
                            &levels,
                            factors,
                            plane,
                            context,
                            lambda,
                            &costs
                        ),
                        reference_score(&coefficients, factors, plane, context, lambda, &costs),
                        "trellis {} {context} {lambda}",
                        plane.index()
                    );
                }
            }
            for magnitude in [0, 1, 3] {
                let coefficients = [magnitude; 16];
                let model = Quantizer::new(factors, 16_384, 16_384, &costs, TrellisMode::Exact);
                let levels = model.quantize(&coefficients, plane, 0);
                assert_eq!(
                    objective(&coefficients, &levels, factors, plane, 0, 16_384, &costs),
                    reference_score(&coefficients, factors, plane, 0, 16_384, &costs),
                    "trellis uniform {magnitude} {}",
                    plane.index()
                );
            }
        }
    }
}
