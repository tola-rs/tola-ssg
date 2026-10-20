//! VP8 coefficient quantization minimizing transform distortion plus token rate.

use std::cell::Cell;

use crate::TrellisMode;
use crate::quantize::Factors;
use crate::token::{COEFF_BANDS, CoefficientPlane, TokenCosts, ZIGZAG};

const CACHED_COEFFICIENTS: usize = 128;
const EDGE_CELLS: usize = 4 * 8 * CACHED_COEFFICIENTS;

/// One plane's token rates at one scan position, scaled by the plane's lambda.
#[derive(Clone, Copy)]
struct PositionRates {
    end: [u32; 3],
    zero: [u32; 3],
    one: [u32; 3],
    edge_base: usize,
}

struct PlaneRates {
    positions: [PositionRates; 17],
    omitted_root: [u32; 16],
    token_rates: [[u32; 12]; 48],
    root_rates: [u32; 48],
    magnitude_rates: Box<[u32; 2049]>,
    token_of: Box<[u8; 2049]>,
}

impl PlaneRates {
    fn new(plane: CoefficientPlane, lambda: u64, costs: &TokenCosts) -> Self {
        let first = plane.first_position();
        let positions = std::array::from_fn(|position| {
            if position == 16 {
                return PositionRates {
                    end: [0; 3],
                    zero: [0; 3],
                    one: [0; 3],
                    edge_base: 0,
                };
            }
            // Band zero is the DC position; every other band shares the plane's AC factor.
            let mut rates = PositionRates {
                end: [0; 3],
                zero: [0; 3],
                one: [0; 3],
                edge_base: (plane.index() * 8 + COEFF_BANDS[position]) * CACHED_COEFFICIENTS,
            };
            for context in 0..3 {
                let previous_was_zero = context == 0 && position != first;
                rates.end[context] =
                    (lambda * u64::from(costs.end(plane, position, context))) as u32;
                rates.zero[context] = (lambda
                    * u64::from(costs.coefficient(plane, position, context, previous_was_zero, 0)))
                    as u32;
                rates.one[context] = (lambda
                    * u64::from(costs.coefficient(plane, position, context, previous_was_zero, 1)))
                    as u32;
            }
            rates
        });
        let omitted_root = std::array::from_fn(|position| {
            if position == first {
                0
            } else {
                (lambda
                    * u64::from(
                        costs.token(plane, position, 0, false, 2)
                            - costs.token(plane, position, 0, true, 2),
                    )) as u32
            }
        });
        let mut token_rates = [[0u32; 12]; 48];
        let mut root_rates = [0u32; 48];
        for position in 0..16 {
            for context in 0..3 {
                let index = position * 3 + context;
                for (token, rate) in token_rates[index].iter_mut().enumerate() {
                    *rate = (lambda
                        * u64::from(costs.token(plane, position, context, false, token as u8)))
                        as u32;
                }
                root_rates[index] =
                    (lambda * u64::from(costs.root(plane, position, context))) as u32;
            }
        }
        let magnitude_rates = Box::new(std::array::from_fn(|magnitude| {
            (lambda * u64::from(costs.magnitude_cost((magnitude as u16).min(2047)))) as u32
        }));
        let token_of = Box::new(std::array::from_fn(|level| {
            crate::token::token_for_level(level as u16)
        }));
        Self {
            positions,
            omitted_root,
            token_rates,
            root_rates,
            magnitude_rates,
            token_of,
        }
    }
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
    factors: Factors,
    luma_lambda: u64,
    chroma_lambda: u64,
    costs: TokenCosts,
    rates: [PlaneRates; 4],
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
        // One analysis frame owns at most 4096 32-byte cells (128 KiB); no cell crosses models.
        let count = if luma_lambda == 0 && chroma_lambda == 0 {
            0
        } else {
            EDGE_CELLS
        };
        let rates = [
            PlaneRates::new(CoefficientPlane::LumaAc, luma_lambda, costs),
            PlaneRates::new(CoefficientPlane::LumaDc, luma_lambda, costs),
            PlaneRates::new(CoefficientPlane::Chroma, chroma_lambda, costs),
            PlaneRates::new(CoefficientPlane::Luma, luma_lambda, costs),
        ];
        Self {
            factors,
            luma_lambda,
            chroma_lambda,
            costs: costs.clone(),
            rates,
            edges: vec![Cell::new(ContextTwoEdges::EMPTY); count].into_boxed_slice(),
            mode,
        }
    }

    pub(crate) fn quantize(
        &self,
        coefficients: &[i32; 16],
        plane: CoefficientPlane,
        first_context: usize,
    ) -> [i16; 16] {
        let (dc, ac, lambda) = match plane {
            CoefficientPlane::LumaAc | CoefficientPlane::Luma => {
                (self.factors.y_dc, self.factors.y_ac, self.luma_lambda)
            }
            CoefficientPlane::LumaDc => (self.factors.y2_dc, self.factors.y2_ac, self.luma_lambda),
            CoefficientPlane::Chroma => (
                self.factors.chroma_dc,
                self.factors.chroma_ac,
                self.chroma_lambda,
            ),
        };
        let rates = match plane {
            CoefficientPlane::LumaAc => &self.rates[0],
            CoefficientPlane::LumaDc => &self.rates[1],
            CoefficientPlane::Chroma => &self.rates[2],
            CoefficientPlane::Luma => &self.rates[3],
        };
        BlockQuantizer {
            dc,
            ac,
            plane,
            lambda,
            costs: &self.costs,
            rates,
            edges: &self.edges,
            mode: self.mode,
        }
        .quantize(coefficients, first_context)
    }
}

struct BlockQuantizer<'a> {
    dc: i32,
    ac: i32,
    plane: CoefficientPlane,
    lambda: u64,
    costs: &'a TokenCosts,
    rates: &'a PlaneRates,
    edges: &'a [Cell<ContextTwoEdges>],
    mode: TrellisMode,
}

impl BlockQuantizer<'_> {
    fn quantize(&self, coefficients: &[i32; 16], first_context: usize) -> [i16; 16] {
        let first = self.plane.first_position();
        if self.lambda == 0 {
            let mut levels = crate::quantize::quantize_block(coefficients, self.dc, self.ac);
            if first == 1 {
                levels[0] = 0;
            }
            return levels;
        }
        let weight = self.plane.distortion_weight();
        // Restricted mode has its own exact zero test in the forward DP scan; the
        // zero-is-nearest maximum scan below only serves the exact DP.
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
            if twice > self.dc as u32 {
                zero_is_nearest = false;
            } else {
                let factor = self.dc as u64;
                minimum_nonzero_penalty = factor * (factor - u64::from(twice)) * weight;
            }
        }
        let mut maximum = 0u32;
        for position in first.max(1)..16 {
            maximum = maximum.max(coefficients[ZIGZAG[position]].unsigned_abs());
        }
        let twice = 2 * maximum;
        if twice > self.ac as u32 {
            zero_is_nearest = false;
        } else {
            let factor = self.ac as u64;
            minimum_nonzero_penalty =
                minimum_nonzero_penalty.min(factor * (factor - u64::from(twice)) * weight);
        }
        // Any nonempty path adds at least this distortion, even if every token were free.
        if zero_is_nearest
            && minimum_nonzero_penalty
                >= self.lambda * u64::from(self.costs.end(self.plane, first, first_context))
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
        let mut choices = [[-1i16; 3]; 16];
        let mut suffix = [u64::MAX, 0, 0];
        let mut tail_error = 0u64;
        // Position 15 has no suffix states yet; peel it so later positions can skip the
        // unreachable-state tests and the context-zero EOB test.
        {
            let position = 15;
            let raster = ZIGZAG[position];
            let factor = self.ac;
            let coefficient = coefficients[raster].unsigned_abs();
            let rates = &self.rates.positions[position];
            let nonunit = self.context_two_edges(position, rates.edge_base, coefficient);
            let error = coefficient as i32;
            let own_error = (error * error) as u64 * weight;
            tail_error += own_error;
            let error = error - factor;
            let one_error = (error * error) as u64 * weight;
            let one_continuation = one_error;
            let omitted_root = self.rates.omitted_root[position];
            let mut prefix = [u64::MAX; 3];
            for context in 0..3 {
                if context != 0 || position == first {
                    prefix[context] = tail_error + u64::from(rates.end[context]);
                }
                if one_continuation != u64::MAX {
                    let score = one_continuation + u64::from(rates.one[context]);
                    if score < prefix[context] {
                        prefix[context] = score;
                        choices[position][context] = 1;
                    }
                }
                let score = nonunit.scores[context]
                    - if context == 0 {
                        u64::from(omitted_root)
                    } else {
                        0
                    }
                    + suffix[2];
                if score < prefix[context] {
                    prefix[context] = score;
                    choices[position][context] = nonunit.levels[context] as i16;
                }
            }
            suffix = prefix;
        }
        for position in (first..15).rev() {
            let raster = ZIGZAG[position];
            let factor = if position == 0 { self.dc } else { self.ac };
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
            let omitted_root = self.rates.omitted_root[position];
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
        for position in first..16 {
            if magnitude < 0 {
                break;
            }
            let raster = ZIGZAG[position];
            levels[raster] = if coefficients[raster] < 0 {
                -magnitude
            } else {
                magnitude
            };
            if position != 15 {
                magnitude = choices[position + 1][usize::from((magnitude as u16).min(2))];
            }
        }
        levels
    }

    fn factor(&self, position: usize) -> i32 {
        if position == 0 { self.dc } else { self.ac }
    }

    /// libwebp-style candidate set: the neutral level, plus one level above it only while
    /// that level is still within the half-biased nearest quantizer.
    fn candidates(&self, magnitude: u32, factor: i32) -> ([u16; 2], usize) {
        let factor = factor as u32;
        let quotient = magnitude / factor;
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
                let index = position * 3 + context;
                let token = usize::from(self.rates.token_of[usize::from(magnitude)]);
                u64::from(
                    self.rates.token_rates[index][token]
                        - if previous_was_zero {
                            self.rates.root_rates[index]
                        } else {
                            0
                        }
                        + self.rates.magnitude_rates[usize::from(magnitude)],
                )
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
        // One reverse scan supplies the zero tail and the last position whose candidate
        // set can be non-zero; a missing last is exactly the all-zero block.
        let mut zero_tail = [0u64; 17];
        let mut last = None;
        for position in (first..16).rev() {
            let magnitude = coefficients[ZIGZAG[position]].unsigned_abs();
            let error = i64::from(magnitude);
            zero_tail[position] = zero_tail[position + 1] + (error * error) as u64 * weight;
            if last.is_none() && 2 * u64::from(magnitude) >= self.factor(position) as u64 {
                last = Some(position);
            }
        }
        let Some(last) = last else {
            return [0; 16];
        };
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
            let (candidates, count) = self.candidates(magnitude, factor);
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
        let factor = if position == 0 { self.dc } else { self.ac };
        let weight = self.plane.distortion_weight();
        let nearest =
            ((2 * u64::from(coefficient) + factor as u64) / (2 * factor as u64)).min(2047) as u16;
        let mut edges = ContextTwoEdges::EMPTY;
        for (token, &(lower, upper)) in MAGNITUDE_RANGES.iter().enumerate().skip(2) {
            let closest = nearest.clamp(lower, upper);
            let error = i64::from(coefficient) - i64::from(closest) * i64::from(factor);
            let bound = (error * error) as u64 * weight
                + self.lambda * u64::from(self.costs.magnitude_cost(lower));
            if bound >= *edges.scores.iter().max().unwrap() {
                continue;
            }
            let mut rates = [0; 3];
            let mut can_improve = false;
            for (context, rate) in rates.iter_mut().enumerate() {
                *rate = self.lambda
                    * u64::from(self.costs.token(
                        self.plane,
                        position,
                        context,
                        false,
                        token as u8,
                    ));
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
                    self.lambda,
                    self.costs,
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
    lambda: u64,
    costs: &TokenCosts,
    (lower, upper): (u16, u16),
    nearest: u16,
) -> (u16, u64) {
    let distortion = |magnitude| {
        let error = i64::from(coefficient) - i64::from(magnitude) * i64::from(factor);
        (error * error) as u64 * weight
    };
    let mut best = (
        nearest,
        distortion(nearest) + lambda * u64::from(costs.magnitude_cost(nearest)),
    );
    // Every category extra-bit probability exceeds 128, so its all-zero offset is cheapest.
    let minimum_rate = lambda * u64::from(costs.magnitude_cost(lower));
    for magnitude in (lower..nearest).rev() {
        let error = distortion(magnitude);
        if error + minimum_rate >= best.1 {
            break;
        }
        let score = error + lambda * u64::from(costs.magnitude_cost(magnitude));
        if score < best.1 {
            best = (magnitude, score);
        }
    }
    for magnitude in nearest + 1..=upper {
        let error = distortion(magnitude);
        if error + minimum_rate >= best.1 {
            break;
        }
        let score = error + lambda * u64::from(costs.magnitude_cost(magnitude));
        if score < best.1 {
            best = (magnitude, score);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{emitted_block_cost, emitted_token_cost};

    fn quantize_block(
        coefficients: &[i32; 16],
        dc: i32,
        ac: i32,
        plane: CoefficientPlane,
        first_context: usize,
        lambda: u64,
        costs: &TokenCosts,
    ) -> [i16; 16] {
        let rates = PlaneRates::new(plane, lambda, costs);
        BlockQuantizer {
            dc,
            ac,
            plane,
            lambda,
            costs,
            rates: &rates,
            edges: &[],
            mode: TrellisMode::Exact,
        }
        .quantize(coefficients, first_context)
    }

    fn probabilities() -> [u8; 1056] {
        std::array::from_fn(|index| (17 + index * 73 % 238) as u8)
    }

    fn category_choices(
        coefficient: u32,
        factor: i32,
        weight: u64,
        lambda: u64,
        costs: &TokenCosts,
    ) -> [(u16, u64); 11] {
        let nearest =
            ((2 * u64::from(coefficient) + factor as u64) / (2 * factor as u64)).min(2047) as u16;
        MAGNITUDE_RANGES.map(|range| {
            category_choice(
                coefficient,
                factor,
                weight,
                lambda,
                costs,
                range,
                nearest.clamp(range.0, range.1),
            )
        })
    }

    fn objective(
        coefficients: &[i32; 16],
        levels: &[i16; 16],
        factors: [i32; 2],
        plane: CoefficientPlane,
        first_context: usize,
        lambda: u64,
        probs: &[u8; 1056],
    ) -> u64 {
        let distortion: u64 = ZIGZAG[plane.first_position()..]
            .iter()
            .map(|raster| {
                let factor = factors[usize::from(*raster != 0)];
                let error = i64::from(coefficients[*raster] - i32::from(levels[*raster]) * factor);
                (error * error) as u64
            })
            .sum();
        distortion * plane.distortion_weight()
            + lambda * emitted_block_cost(levels, plane, first_context, probs)
    }

    fn exhaustive_score(
        coefficients: &[i32; 16],
        factors: [i32; 2],
        plane: CoefficientPlane,
        first_context: usize,
        lambda: u64,
        probs: &[u8; 1056],
    ) -> u64 {
        let first = plane.first_position();
        let mut prefixes = [u64::MAX; 3];
        prefixes[first_context] = 0;
        let mut minimum = u64::MAX;
        for position in first..16 {
            let mut next = [u64::MAX; 3];
            let tail_error: u64 = ZIGZAG[position..]
                .iter()
                .map(|raster| {
                    let coefficient = i64::from(coefficients[*raster]);
                    (coefficient * coefficient) as u64 * plane.distortion_weight()
                })
                .sum();
            for (context, prefix) in prefixes.into_iter().enumerate() {
                if prefix == u64::MAX {
                    continue;
                }
                if position == first || context != 0 {
                    minimum = minimum.min(
                        prefix
                            + tail_error
                            + lambda
                                * emitted_token_cost(plane, position, context, false, None, probs),
                    );
                }
                for magnitude in 0..=2047u16 {
                    let raster = ZIGZAG[position];
                    let error = i64::from(coefficients[raster].unsigned_abs())
                        - i64::from(magnitude) * i64::from(factors[usize::from(raster != 0)]);
                    let score = prefix
                        + (error * error) as u64 * plane.distortion_weight()
                        + lambda
                            * emitted_token_cost(
                                plane,
                                position,
                                context,
                                position != first && context == 0,
                                Some(magnitude),
                                probs,
                            );
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

    #[test]
    fn trellis_matches_full_magnitude_search() {
        let factors = Factors {
            y_dc: 11,
            y_ac: 8,
            y2_dc: 11,
            y2_ac: 8,
            chroma_dc: 11,
            chroma_ac: 8,
        };
        for probs in [probabilities(), crate::tables::DEFAULT_COEFF_PROBS] {
            let costs = TokenCosts::new(&probs);
            let models = [
                Quantizer::new(factors, 1024, 1024, &costs, TrellisMode::Exact),
                Quantizer::new(factors, 258_064, 258_064, &costs, TrellisMode::Exact),
            ];
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
                    for (lambda, model) in [1024, 258_064].into_iter().zip(&models) {
                        let levels =
                            quantize_block(&coefficients, 11, 8, plane, context, lambda, &costs);
                        assert_eq!(model.quantize(&coefficients, plane, context), levels);
                        assert_eq!(
                            objective(
                                &coefficients,
                                &levels,
                                [11, 8],
                                plane,
                                context,
                                lambda,
                                &probs
                            ),
                            exhaustive_score(
                                &coefficients,
                                [11, 8],
                                plane,
                                context,
                                lambda,
                                &probs
                            )
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn frame_quantizers_bind_score_models() {
        let coefficients = [
            12, -79, 127, -127, 0, 3, -4, 18, 40, -66, 128, -128, 2040, -2040, 7, -11,
        ];
        for index in [8, 32] {
            let factors = crate::quantize::factors(index);
            for probs in [probabilities(), crate::tables::DEFAULT_COEFF_PROBS] {
                let costs = TokenCosts::new(&probs);
                let model = Quantizer::new(factors, 1024, 64, &costs, TrellisMode::Exact);
                for _ in 0..2 {
                    for (plane, dc, ac, lambda) in [
                        (CoefficientPlane::LumaAc, factors.y_dc, factors.y_ac, 1024),
                        (CoefficientPlane::LumaDc, factors.y2_dc, factors.y2_ac, 1024),
                        (
                            CoefficientPlane::Chroma,
                            factors.chroma_dc,
                            factors.chroma_ac,
                            64,
                        ),
                        (CoefficientPlane::Luma, factors.y_dc, factors.y_ac, 1024),
                    ] {
                        for context in 0..3 {
                            assert_eq!(
                                model.quantize(&coefficients, plane, context),
                                quantize_block(
                                    &coefficients,
                                    dc,
                                    ac,
                                    plane,
                                    context,
                                    lambda,
                                    &costs
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn category_bounds_preserve_full_search() {
        let costs = TokenCosts::new(&probabilities());
        for coefficient in [0, 3, 44, 1462, 2060, 16_320] {
            for factor in [4, 8, 22, 58, 284] {
                for weight in [4096, 16384] {
                    for lambda in [1, 1024, 16_384, 258_064] {
                        let selected =
                            category_choices(coefficient, factor, weight, lambda, &costs);
                        for ((lower, upper), (_, score)) in
                            MAGNITUDE_RANGES.into_iter().zip(selected)
                        {
                            let minimum = (lower..=upper)
                                .map(|magnitude| {
                                    let error = i64::from(coefficient)
                                        - i64::from(magnitude) * i64::from(factor);
                                    (error * error) as u64 * weight
                                        + lambda * u64::from(costs.magnitude_cost(magnitude))
                                })
                                .min()
                                .unwrap();
                            assert_eq!(score, minimum);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn empty_block_bound_preserves_full_search() {
        let probs = probabilities();
        let costs = TokenCosts::new(&probs);
        let plane = CoefficientPlane::Chroma;
        for coefficient in [0, 1, 3, 17] {
            let coefficients = [coefficient; 16];
            let levels = quantize_block(&coefficients, 36, 36, plane, 0, 16_384, &costs);
            assert_eq!(
                objective(&coefficients, &levels, [36; 2], plane, 0, 16_384, &probs),
                exhaustive_score(&coefficients, [36; 2], plane, 0, 16_384, &probs)
            );
        }
    }

    #[test]
    fn category_bounds_match_full_search() {
        let probs = crate::tables::DEFAULT_COEFF_PROBS;
        let costs = TokenCosts::new(&probs);
        for (coefficient, factor, context, lambda, scalar, expected) in
            [(1462, 22, 0, 1024, 66, 67), (2060, 58, 1, 16_384, 36, 34)]
        {
            let mut coefficients = [0; 16];
            coefficients[0] = coefficient;
            let plane = CoefficientPlane::LumaDc;
            let selected = quantize_block(
                &coefficients,
                factor,
                factor,
                plane,
                context,
                lambda,
                &costs,
            );
            let mut restricted = [0; 16];
            restricted[0] = scalar;
            assert_eq!(selected[0], expected);
            assert!(
                objective(
                    &coefficients,
                    &selected,
                    [factor; 2],
                    plane,
                    context,
                    lambda,
                    &probs
                ) < objective(
                    &coefficients,
                    &restricted,
                    [factor; 2],
                    plane,
                    context,
                    lambda,
                    &probs
                )
            );
        }
    }

    #[test]
    fn zero_lambda_preserves_nearest_rounding() {
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

    #[test]
    fn coefficient_weights_match_pixel_energy() {
        let mut dct = [0; 16];
        dct[0] = 64;
        let pixel_energy: u64 = crate::transform::inverse_dct(fearless_simd::Fallback::new(), &dct)
            .into_iter()
            .map(|sample| (sample * sample) as u64)
            .sum();
        assert_eq!(
            64 * 64 * CoefficientPlane::Chroma.distortion_weight(),
            pixel_energy * 65536
        );

        for position in 0..16 {
            let mut wht = [0; 16];
            wht[position] = 256;
            let pixel_energy: u64 = crate::transform::inverse_wht(&wht)
                .into_iter()
                .map(|dc| {
                    let mut dct = [0; 16];
                    dct[0] = dc;
                    crate::transform::inverse_dct(fearless_simd::Fallback::new(), &dct)
                        .into_iter()
                        .map(|sample| (sample * sample) as u64)
                        .sum::<u64>()
                })
                .sum();
            assert_eq!(
                256 * 256 * CoefficientPlane::LumaDc.distortion_weight(),
                pixel_energy * 65536
            );
        }
    }
}
