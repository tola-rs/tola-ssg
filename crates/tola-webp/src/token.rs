//! VP8 coefficient tokens, scan order, contexts, and extra bits (RFC 6386 §13).

#[cfg(test)]
use crate::bool_writer::tree_path;
use crate::bool_writer::{BoolWriter, LengthWriter};
use crate::cost::{CostWriter, bool_cost};
use crate::macroblock::{CoefficientSpool, Luma, Macroblock, PackedLevels};
use crate::tables::{COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS};
use std::sync::{Arc, LazyLock};

pub(crate) const BLOCK_COEFFICIENTS: usize = 16;
pub(crate) const CONTEXTS: usize = 3;
pub(crate) const BANDS: usize = 8;
pub(crate) const TOKENS: usize = 12;
pub(crate) const ENTRIES: usize = 96;
pub(crate) const PROBABILITIES_PER_ENTRY: usize = 11;
pub(crate) const COEFFICIENT_PROBABILITIES: usize = ENTRIES * PROBABILITIES_PER_ENTRY;
pub(crate) const MAGNITUDES: usize = 2048;
pub(crate) const SMALL_MAGNITUDES: usize = 128;
pub(crate) const PAIR_MAGNITUDES: usize = 32;
pub(crate) const PAIR_ENTRIES: usize = PAIR_MAGNITUDES * PAIR_MAGNITUDES;

const COEFF_TREE: [i8; 22] = [
    -11, 2, 0, 4, -1, 6, 8, 12, -2, 10, -3, -4, 14, 16, -5, -6, 18, 20, -7, -8, -9, -10,
];

type TokenStep = (u8, bool);

#[derive(Clone, Copy)]
struct TokenPath {
    steps: [TokenStep; 7],
    len: u8,
}

impl TokenPath {
    fn branches(&self) -> &[TokenStep] {
        &self.steps[..usize::from(self.len)]
    }
}

const COEFF_PATHS: [[TokenPath; TOKENS]; 2] = [
    coefficient_paths(&COEFF_TREE, 0),
    coefficient_paths(&COEFF_TREE, 2),
];

const fn coefficient_paths<const N: usize>(tree: &[i8], start: usize) -> [TokenPath; N] {
    const fn fill<const N: usize>(
        tree: &[i8],
        node: usize,
        len: usize,
        steps: [TokenStep; 7],
        paths: &mut [TokenPath; N],
    ) {
        assert!(len < 7, "VP8 coefficient token paths fit seven branches");
        let mut branch = 0;
        while branch < 2 {
            let child = tree[node + branch];
            let mut steps = steps;
            steps[len] = ((node >> 1) as u8, branch != 0);
            if child <= 0 {
                paths[child.unsigned_abs() as usize] = TokenPath {
                    steps,
                    len: (len + 1) as u8,
                };
            } else {
                fill(tree, child as usize, len + 1, steps, paths);
            }
            branch += 1;
        }
    }
    let mut paths = [TokenPath {
        steps: [(0, false); 7],
        len: 0,
    }; N];
    fill(tree, start, 0, [(0, false); 7], &mut paths);
    paths
}

pub(crate) const COEFF_BANDS: [usize; BLOCK_COEFFICIENTS] =
    [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7];

const ENTRY_BASE: [[u16; BLOCK_COEFFICIENTS]; 4] = {
    let mut table = [[0u16; BLOCK_COEFFICIENTS]; 4];
    let mut plane = 0;
    while plane < table.len() {
        let mut pos = 0;
        while pos < BLOCK_COEFFICIENTS {
            table[plane][pos] = band_entry(plane, COEFF_BANDS[pos], 0) as u16;
            pos += 1;
        }
        plane += 1;
    }
    table
};

pub(crate) const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

const PAIR_SCAN: [[u8; BLOCK_COEFFICIENTS / 2]; 2] = {
    let mut scan = [[0; BLOCK_COEFFICIENTS / 2]; 2];
    let mut first = 0;
    while first < scan.len() {
        let mut pos = first;
        while pos + 1 < BLOCK_COEFFICIENTS {
            scan[first][pos / 2] = (ZIGZAG[pos] | (ZIGZAG[pos + 1] << 4)) as u8;
            pos += 2;
        }
        first += 1;
    }
    scan
};

const fn band_entry(plane: usize, band: usize, ctx: usize) -> usize {
    (plane * BANDS + band) * CONTEXTS + ctx
}

const CATEGORY_ONE: [u8; 1] = [159];
const CATEGORY_TWO: [u8; 2] = [165, 145];
const CATEGORY_THREE: [u8; 3] = [173, 148, 140];
const CATEGORY_FOUR: [u8; 4] = [176, 155, 140, 135];
const CATEGORY_FIVE: [u8; 5] = [180, 157, 141, 134, 130];
const CATEGORY_SIX: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

const PLANE_Y_AFTER_Y2: usize = 0;
const PLANE_Y2: usize = 1;
const PLANE_CHROMA: usize = 2;
const PLANE_Y_WITHOUT_Y2: usize = 3;

#[derive(Clone, Copy)]
pub(crate) enum CoefficientPlane {
    LumaAc,
    LumaDc,
    Chroma,
    Luma,
}

impl CoefficientPlane {
    pub(crate) const ALL: [Self; 4] = [Self::LumaAc, Self::LumaDc, Self::Chroma, Self::Luma];

    pub(crate) fn first_position(self) -> usize {
        usize::from(matches!(self, Self::LumaAc))
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::LumaAc => PLANE_Y_AFTER_Y2,
            Self::LumaDc => PLANE_Y2,
            Self::Chroma => PLANE_CHROMA,
            Self::Luma => PLANE_Y_WITHOUT_Y2,
        }
    }

    pub(crate) fn distortion_weight(self) -> u64 {
        // DCT energy is 4x pixel energy; Y2 WHT adds another factor of 4.
        if matches!(self, Self::LumaDc) {
            4096
        } else {
            16384
        }
    }
}

#[derive(Clone)]
pub(crate) struct TokenCosts {
    tokens: [[u16; TOKENS]; ENTRIES],
    roots: [u16; ENTRIES],
    mags: &'static [u16; MAGNITUDES],
    small_coeff: Arc<[[u16; SMALL_MAGNITUDES]; ENTRIES]>,
    // Masked to 127 in hot loop to elide bounds checks.
    coeff_pairs: Arc<[[u16; PAIR_ENTRIES]; ENTRIES]>,
}

static MAGNITUDE_COSTS: LazyLock<[u16; MAGNITUDES]> = LazyLock::new(|| {
    std::array::from_fn(|mag| {
        if mag == 0 {
            return 0;
        }
        let mut w = CostWriter::new();
        let tok = token_for_magnitude(mag as u16);
        write_extra_bits(&mut w, mag as u16, tok);
        w.write_bool(128, false);
        w.bits() as u16
    })
});

static DEFAULT_TOKEN_COSTS: LazyLock<TokenCosts> =
    LazyLock::new(|| TokenCosts::new(&DEFAULT_COEFF_PROBS));

impl Default for TokenCosts {
    fn default() -> Self {
        DEFAULT_TOKEN_COSTS.clone()
    }
}

struct WideScan<'a> {
    levels: &'a [i16; 16],
    plane: CoefficientPlane,
    first: usize,
    rate: u64,
    pos: usize,
    ctx: usize,
    last: usize,
}

impl TokenCosts {
    pub(crate) fn new(probs: &[u8; COEFFICIENT_PROBABILITIES]) -> Self {
        let mut tokens = [[0; TOKENS]; ENTRIES];
        let mut roots = [0; ENTRIES];
        let mags = &*MAGNITUDE_COSTS;
        for entry in 0..ENTRIES {
            let p = &probs[entry * PROBABILITIES_PER_ENTRY..(entry + 1) * PROBABILITIES_PER_ENTRY];
            roots[entry] = bool_cost(p[0], true) as u16;
            tokens[entry] = COEFF_PATHS[0].map(|path| {
                path.branches()
                    .iter()
                    .map(|&(off, branch)| bool_cost(p[usize::from(off)], branch) as u16)
                    .sum()
            });
        }
        let small_coeff: Arc<[[u16; SMALL_MAGNITUDES]; ENTRIES]> =
            Arc::new(std::array::from_fn(|entry| {
                std::array::from_fn(|mag| {
                    tokens[entry][usize::from(token_for_magnitude(mag as u16))] + mags[mag]
                })
            }));
        let pairs: Arc<[[u16; PAIR_ENTRIES]]> = (0..ENTRIES)
            .map(|entry| {
                let ctx = entry % CONTEXTS;
                let group = entry / CONTEXTS % BANDS;
                let plane = entry / (CONTEXTS * BANDS);
                let pos = usize::from(plane == PLANE_Y_AFTER_Y2) + 2 * group;
                std::array::from_fn(|packed| {
                    if pos + 1 >= 16 {
                        return 0;
                    }
                    let first = packed & (PAIR_MAGNITUDES - 1);
                    let second = packed >> PAIR_MAGNITUDES.trailing_zeros();
                    let e0 = band_entry(plane, COEFF_BANDS[pos], ctx);
                    let e1 = band_entry(plane, COEFF_BANDS[pos + 1], first.min(2));
                    let c0 =
                        small_coeff[e0][first] - if group != 0 && ctx == 0 { roots[e0] } else { 0 };
                    let c1 = small_coeff[e1][second] - if first == 0 { roots[e1] } else { 0 };
                    c0 + c1
                })
            })
            .collect();
        let coeff_pairs = pairs
            .try_into()
            .expect("the pair table has a fixed row count");
        Self {
            tokens,
            roots,
            mags,
            small_coeff,
            coeff_pairs,
        }
    }

    #[inline(always)]
    pub(crate) fn coefficient(
        &self,
        plane: CoefficientPlane,
        pos: usize,
        ctx: usize,
        prev_zero: bool,
        mag: u16,
    ) -> u32 {
        let entry = usize::from(ENTRY_BASE[plane.index()][pos]) + ctx;
        let rate = if mag < SMALL_MAGNITUDES as u16 {
            u32::from(self.small_coeff[entry][usize::from(mag)])
        } else {
            let tok = usize::from(token_for_magnitude(mag));
            u32::from(self.tokens[entry][tok]) + self.magnitude_cost(mag)
        };
        rate - if prev_zero {
            u32::from(self.roots[entry])
        } else {
            0
        }
    }

    pub(crate) fn band(
        &self,
        plane: CoefficientPlane,
        band: usize,
        ctx: usize,
    ) -> (u16, [u16; TOKENS]) {
        let entry = band_entry(plane.index(), band, ctx);
        (self.roots[entry], self.tokens[entry])
    }

    pub(crate) fn magnitude_cost(&self, mag: u16) -> u32 {
        u32::from(self.mags[usize::from(mag)])
    }

    #[inline(always)]
    pub(crate) fn end(&self, plane: CoefficientPlane, pos: usize, ctx: usize) -> u32 {
        let entry = usize::from(ENTRY_BASE[plane.index()][pos]) + ctx;
        u32::from(self.tokens[entry][usize::from(TOKEN_END)])
    }

    pub(crate) fn block(&self, levels: &[i16; 16], plane: CoefficientPlane, ctx0: usize) -> u64 {
        self.block_rate(levels, plane, ctx0).0
    }

    fn block_rate(&self, levels: &[i16; 16], plane: CoefficientPlane, ctx0: usize) -> (u64, bool) {
        let first = plane.first_position();
        let Some(last) = last_nonzero(levels, first) else {
            return (u64::from(self.end(plane, first, ctx0)), false);
        };
        let scan = &PAIR_SCAN[first & 1];
        let mut pair_entry = plane.index() * (CONTEXTS * BANDS);
        let mut rate = 0;
        let mut ctx = ctx0;
        let mut pos = first;
        for &offsets in scan {
            if pos >= last {
                break;
            }
            let m0 = levels[usize::from(offsets & 15)].unsigned_abs();
            let m1 = levels[usize::from(offsets >> 4)].unsigned_abs();
            if usize::from(m0 | m1) >= PAIR_MAGNITUDES {
                return self.block_rate_wide(WideScan {
                    levels,
                    plane,
                    first,
                    rate,
                    pos,
                    ctx,
                    last,
                });
            }
            let packed = (usize::from(m0) | (usize::from(m1) << PAIR_MAGNITUDES.trailing_zeros()))
                & (PAIR_ENTRIES - 1);
            rate += u64::from(self.coeff_pairs[pair_entry + ctx][packed]);
            pair_entry += CONTEXTS;
            pos += 2;
            ctx = usize::from(m1.min(2));
        }
        if pos == last {
            let mag = levels[ZIGZAG[pos]].unsigned_abs();
            let prev_zero = pos != first && levels[ZIGZAG[pos - 1]] == 0;
            rate += u64::from(self.coefficient(plane, pos, ctx, prev_zero, mag));
            ctx = mag_context(mag);
        }
        if last < 15 {
            rate += u64::from(self.end(plane, last + 1, ctx));
        }
        (rate, true)
    }

    #[inline(never)]
    fn block_rate_wide(&self, scan: WideScan<'_>) -> (u64, bool) {
        let WideScan {
            levels,
            plane,
            first,
            mut rate,
            mut pos,
            mut ctx,
            last,
        } = scan;
        let mut prev_zero = pos != first && levels[ZIGZAG[pos - 1]] == 0;
        while pos < last {
            let m0 = levels[ZIGZAG[pos]].unsigned_abs();
            let m1 = levels[ZIGZAG[pos + 1]].unsigned_abs();
            rate += u64::from(self.coefficient(plane, pos, ctx, prev_zero, m0));
            rate += u64::from(self.coefficient(plane, pos + 1, mag_context(m0), m0 == 0, m1));
            prev_zero = m1 == 0;
            ctx = mag_context(m1);
            pos += 2;
        }
        if pos == last {
            let mag = levels[ZIGZAG[pos]].unsigned_abs();
            rate += u64::from(self.coefficient(plane, pos, ctx, prev_zero, mag));
            ctx = mag_context(mag);
        }
        if last < 15 {
            rate += u64::from(self.end(plane, last + 1, ctx));
        }
        (rate, true)
    }
}

#[derive(Default, Clone, Copy)]
pub(crate) struct Edges {
    // I4 has no Y2 token and preserves the previous Y2 context.
    y2: bool,
    y: [bool; 4],
    u: [bool; 2],
    v: [bool; 2],
}

#[derive(Default, Clone, Copy)]
pub(crate) struct MacroblockContexts {
    above: Edges,
    left: Edges,
}

impl MacroblockContexts {
    pub(crate) fn empty_rates(&self, costs: &TokenCosts) -> (u64, u64) {
        let end = |plane: CoefficientPlane| {
            std::array::from_fn(|ctx| u64::from(costs.end(plane, plane.first_position(), ctx)))
        };
        let y2 = u64::from(costs.end(CoefficientPlane::LumaDc, 0, self.y2()));
        let luma = y2 + empty_grid_rate(end(CoefficientPlane::LumaAc), &self.above.y, &self.left.y);
        let chroma = end(CoefficientPlane::Chroma);
        let chroma = empty_grid_rate(chroma, &self.above.u, &self.left.u)
            + empty_grid_rate(chroma, &self.above.v, &self.left.v);
        (luma, chroma)
    }

    pub(crate) fn y2(&self) -> usize {
        usize::from(self.above.y2) + usize::from(self.left.y2)
    }

    pub(crate) fn set_y2(&mut self, nz: bool) {
        self.above.y2 = nz;
        self.left.y2 = nz;
    }

    pub(crate) fn y(&self, i: usize) -> usize {
        usize::from(self.above.y[i % 4]) + usize::from(self.left.y[i / 4])
    }

    pub(crate) fn set_y(&mut self, i: usize, nz: bool) {
        self.above.y[i % 4] = nz;
        self.left.y[i / 4] = nz;
    }

    pub(crate) fn u(&self, i: usize) -> usize {
        usize::from(self.above.u[i % 2]) + usize::from(self.left.u[i / 2])
    }

    pub(crate) fn set_u(&mut self, i: usize, nz: bool) {
        self.above.u[i % 2] = nz;
        self.left.u[i / 2] = nz;
    }

    pub(crate) fn v(&self, i: usize) -> usize {
        usize::from(self.above.v[i % 2]) + usize::from(self.left.v[i / 2])
    }

    pub(crate) fn set_v(&mut self, i: usize, nz: bool) {
        self.above.v[i % 2] = nz;
        self.left.v[i / 2] = nz;
    }

    #[cfg(test)]
    pub(crate) fn luma_cost(
        &mut self,
        costs: &TokenCosts,
        y2: Option<&[i16; 16]>,
        y: &[[i16; 16]; 16],
    ) -> u64 {
        let mut rate = 0;
        let plane = if let Some(y2) = y2 {
            let (blk_rate, nz) = costs.block_rate(y2, CoefficientPlane::LumaDc, self.y2());
            rate += blk_rate;
            self.set_y2(nz);
            CoefficientPlane::LumaAc
        } else {
            CoefficientPlane::Luma
        };
        for (i, blk) in y.iter().enumerate() {
            let (blk_rate, nz) = costs.block_rate(blk, plane, self.y(i));
            rate += blk_rate;
            self.set_y(i, nz);
        }
        rate
    }

    pub(crate) fn chroma_cost(
        &mut self,
        costs: &TokenCosts,
        u: &[[i16; 16]; 4],
        v: &[[i16; 16]; 4],
    ) -> u64 {
        let mut rate = 0;
        for (i, blk) in u.iter().enumerate() {
            let (blk_rate, nz) = costs.block_rate(blk, CoefficientPlane::Chroma, self.u(i));
            rate += blk_rate;
            self.set_u(i, nz);
        }
        for (i, blk) in v.iter().enumerate() {
            let (blk_rate, nz) = costs.block_rate(blk, CoefficientPlane::Chroma, self.v(i));
            rate += blk_rate;
            self.set_v(i, nz);
        }
        rate
    }
}

fn empty_grid_rate<const N: usize>(
    rates: [u64; CONTEXTS],
    above: &[bool; N],
    left: &[bool; N],
) -> u64 {
    let first = usize::from(above[0]) + usize::from(left[0]);
    let ones = above[1..]
        .iter()
        .chain(&left[1..])
        .map(|&nz| u64::from(nz))
        .sum::<u64>();
    let zeros = (N * N - 1) as u64 - ones;
    rates[first] + ones * rates[1] + zeros * rates[0]
}

pub(crate) trait TokenSink {
    fn write_bool(&mut self, prob: u8, value: bool);
    #[inline(always)]
    fn coefficient_branch(&mut self, _index: usize, prob: u8, value: bool) {
        self.write_bool(prob, value);
    }
    fn counts_only(&self) -> bool {
        false
    }
    #[inline(always)]
    fn coefficient_token(&mut self, entry: usize, tok: u8, skip_end: bool, probs: &[u8; 1056])
    where
        Self: Sized,
    {
        let path = &COEFF_PATHS[usize::from(skip_end)][usize::from(tok)];
        write_path(self, path, entry * 11, probs);
    }
}

impl TokenSink for BoolWriter {
    #[inline(always)]
    fn write_bool(&mut self, prob: u8, value: bool) {
        BoolWriter::write_bool(self, prob, value);
    }
}

impl TokenSink for CostWriter {
    fn write_bool(&mut self, prob: u8, value: bool) {
        CostWriter::write_bool(self, prob, value);
    }
}

pub(crate) const TOKEN_END: u8 = 11;

pub(crate) type BranchCounts = Box<[[u32; 2]; 1056]>;

pub(crate) struct TokenCounts {
    tokens: Box<[[u32; 16]; 256]>,
}

impl TokenCounts {
    pub(crate) fn new() -> Self {
        Self {
            tokens: Box::new([[0; 16]; 256]),
        }
    }

    pub(crate) fn into_counts(self) -> BranchCounts {
        let mut counts = Box::new([[0; 2]; 1056]);
        for (slot, hist) in self.tokens[..192].iter().enumerate() {
            let entry = slot / 2;
            let skip_end = slot % 2;
            for (tok, &n) in hist.iter().enumerate() {
                if n == 0 {
                    continue;
                }
                for &(off, branch) in COEFF_PATHS[skip_end][tok].branches() {
                    counts[entry * 11 + usize::from(off)][usize::from(branch)] += n;
                }
            }
        }
        counts
    }
}

impl TokenSink for TokenCounts {
    fn write_bool(&mut self, _prob: u8, _value: bool) {}

    fn counts_only(&self) -> bool {
        true
    }

    fn coefficient_token(&mut self, entry: usize, tok: u8, skip_end: bool, _probs: &[u8; 1056]) {
        // Power-of-two masks keep both indices in bounds.
        self.tokens[(entry * 2 + usize::from(skip_end)) & 255][usize::from(tok) & 15] += 1;
    }
}

/// Emits fitted residual tokens while tracking the default-table partition length.
pub(crate) struct FittedPartition<'a> {
    w: BoolWriter,
    default_len: LengthWriter,
    fitted_probs: &'a [u8; 1056],
}

impl<'a> FittedPartition<'a> {
    pub(crate) fn new(fitted_probs: &'a [u8; 1056]) -> Self {
        Self {
            w: BoolWriter::new(),
            default_len: LengthWriter::new(),
            fitted_probs,
        }
    }

    pub(crate) fn finish(self) -> (Vec<u8>, usize) {
        (self.w.finish(), self.default_len.finish())
    }
}

impl TokenSink for FittedPartition<'_> {
    fn write_bool(&mut self, prob: u8, value: bool) {
        self.w.write_bool(prob, value);
        self.default_len.write_bool(prob, value);
    }

    fn coefficient_branch(&mut self, idx: usize, prob: u8, value: bool) {
        debug_assert_eq!(prob, self.fitted_probs[idx]);
        self.w.write_bool(prob, value);
        self.default_len.write_bool(DEFAULT_COEFF_PROBS[idx], value);
    }
}

#[derive(Clone)]
pub(crate) struct TokenContexts {
    above: Vec<Edges>,
    left: Edges,
}

impl TokenContexts {
    pub(crate) fn new(mb_cols: usize) -> Self {
        Self {
            above: vec![Edges::default(); mb_cols],
            left: Edges::default(),
        }
    }

    pub(crate) fn contexts(&self, mb_x: usize) -> MacroblockContexts {
        MacroblockContexts {
            above: self.above[mb_x],
            left: if mb_x == 0 {
                Edges::default()
            } else {
                self.left
            },
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn left(&self) -> Edges {
        self.left
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_left(&mut self, left: Edges) {
        self.left = left;
    }

    pub(crate) fn advance(&mut self, mb_x: usize, mb: &Macroblock) {
        let mut ctx = self.contexts(mb_x);
        let first = match &mb.luma {
            Luma::Intra16 { .. } => {
                ctx.set_y2(mb.lengths[0] != 0);
                1
            }
            Luma::Intra4 { .. } => 0,
        };
        for (i, &len) in mb.lengths[1..17].iter().enumerate() {
            ctx.set_y(i, usize::from(len) > first);
        }
        for (i, &len) in mb.lengths[17..21].iter().enumerate() {
            ctx.set_u(i, len != 0);
        }
        for (i, &len) in mb.lengths[21..].iter().enumerate() {
            ctx.set_v(i, len != 0);
        }
        self.commit(mb_x, ctx);
    }

    fn commit(&mut self, mb_x: usize, ctx: MacroblockContexts) {
        self.above[mb_x] = ctx.above;
        self.left = ctx.left;
    }
}

#[derive(Clone)]
pub(crate) struct TokenWriter {
    probs: [u8; 1056],
    contexts: TokenContexts,
}

impl TokenWriter {
    pub(crate) fn new(mb_cols: usize) -> Self {
        Self::with_probs(mb_cols, DEFAULT_COEFF_PROBS)
    }

    pub(crate) fn with_probs(mb_cols: usize, probs: [u8; 1056]) -> Self {
        Self {
            probs,
            contexts: TokenContexts::new(mb_cols),
        }
    }

    pub(crate) fn skip_macroblock(&mut self, mb_x: usize, luma: &Luma) {
        let mut ctx = MacroblockContexts::default();
        if matches!(luma, Luma::Intra4 { .. }) {
            let prev = self.contexts.contexts(mb_x);
            ctx.above.y2 = prev.above.y2;
            ctx.left.y2 = prev.left.y2;
        }
        self.contexts.commit(mb_x, ctx);
    }

    #[inline]
    pub(crate) fn write_macroblock<W: TokenSink>(
        &mut self,
        w: &mut W,
        mb_x: usize,
        mb: &Macroblock,
        spool: &CoefficientSpool,
    ) {
        let mut ctx = self.contexts.contexts(mb_x);
        match spool.levels(mb) {
            PackedLevels::Narrow(levels) => self.write_blocks(w, levels, mb, &mut ctx),
            PackedLevels::Wide(levels) => self.write_blocks(w, levels, mb, &mut ctx),
        }
        self.contexts.commit(mb_x, ctx);
    }

    fn write_blocks<W: TokenSink, C: Copy + Into<i16>>(
        &self,
        w: &mut W,
        mut levels: &[C],
        mb: &Macroblock,
        ctx: &mut MacroblockContexts,
    ) {
        let lengths = &mb.lengths;
        let plane = match mb.luma {
            Luma::Intra4 { .. } => CoefficientPlane::Luma,
            Luma::Intra16 { .. } => {
                let (y2, rest) = levels.split_at(usize::from(lengths[0]));
                levels = rest;
                let nz = write_block(w, y2, CoefficientPlane::LumaDc, ctx.y2(), &self.probs);
                ctx.set_y2(nz);
                CoefficientPlane::LumaAc
            }
        };
        for (i, &len) in lengths[1..17].iter().enumerate() {
            let (blk, rest) = levels.split_at(usize::from(len));
            levels = rest;
            let nz = write_block(w, blk, plane, ctx.y(i), &self.probs);
            ctx.set_y(i, nz);
        }
        for (base, above, left) in [
            (17, &mut ctx.above.u, &mut ctx.left.u),
            (21, &mut ctx.above.v, &mut ctx.left.v),
        ] {
            for (i, &len) in lengths[base..base + 4].iter().enumerate() {
                let col = i % 2;
                let row = i / 2;
                let blk_ctx = usize::from(above[col]) + usize::from(left[row]);
                let (blk, rest) = levels.split_at(usize::from(len));
                levels = rest;
                let nz = write_block(w, blk, CoefficientPlane::Chroma, blk_ctx, &self.probs);
                above[col] = nz;
                left[row] = nz;
            }
        }
    }
}

#[inline]
pub(crate) fn last_nonzero(levels: &[i16; 16], first: usize) -> Option<usize> {
    let at = |pos: usize| levels[ZIGZAG[pos]];
    if at(8) | at(9) | at(10) | at(11) | at(12) | at(13) | at(14) | at(15) != 0 {
        return (8..16).rev().find(|&pos| at(pos) != 0);
    }
    if at(4) | at(5) | at(6) | at(7) != 0 {
        return (4..8).rev().find(|&pos| at(pos) != 0);
    }
    if at(2) | at(3) != 0 {
        return Some(if at(3) != 0 { 3 } else { 2 });
    }
    if at(1) != 0 {
        return Some(1);
    }
    (first == 0 && at(0) != 0).then_some(0)
}

#[inline(always)]
fn write_block<W: TokenSink, C: Copy + Into<i16>>(
    w: &mut W,
    levels: &[C],
    plane: CoefficientPlane,
    ctx0: usize,
    probs: &[u8; 1056],
) -> bool {
    let first = plane.first_position();
    let plane = plane.index();
    if levels.len() <= first {
        write_token(w, TOKEN_END, (plane, first, ctx0), false, probs);
        return false;
    }

    let last = levels.len() - 1;
    let mut ctx = ctx0;
    let mut prev_zero = false;
    for (pos, &level) in levels.iter().enumerate().skip(first) {
        let level = level.into();
        let mag = level.unsigned_abs();
        let tok = token_for_magnitude(mag);
        write_token(w, tok, (plane, pos, ctx), prev_zero, probs);
        if mag != 0 && !w.counts_only() {
            write_extra_bits(w, mag, tok);
            w.write_bool(128, level < 0);
        }
        ctx = mag_context(mag);
        prev_zero = mag == 0;
    }

    if last < 15 {
        write_token(w, TOKEN_END, (plane, last + 1, ctx), false, probs);
    }
    true
}

#[inline(always)]
fn write_path<W: TokenSink>(w: &mut W, path: &TokenPath, base: usize, probs: &[u8]) {
    for &(off, branch) in path.branches() {
        let idx = base + usize::from(off);
        w.coefficient_branch(idx, probs[idx], branch);
    }
}

#[inline(always)]
fn write_token<W: TokenSink>(
    w: &mut W,
    tok: u8,
    (plane, pos, ctx): (usize, usize, usize),
    skip_end: bool,
    probs: &[u8; 1056],
) {
    let entry = usize::from(ENTRY_BASE[plane][pos]) + ctx;
    debug_assert_ne!(
        COEFF_PATHS[usize::from(skip_end)][usize::from(tok)].len,
        0,
        "the token must be a tree leaf"
    );
    w.coefficient_token(entry, tok, skip_end, probs);
}

fn write_extra_bits<W: TokenSink>(w: &mut W, mag: u16, tok: u8) {
    let (base, probs): (u16, &[u8]) = match tok {
        5 => (5, &CATEGORY_ONE),
        6 => (7, &CATEGORY_TWO),
        7 => (11, &CATEGORY_THREE),
        8 => (19, &CATEGORY_FOUR),
        9 => (35, &CATEGORY_FIVE),
        10 => (67, &CATEGORY_SIX),
        _ => return,
    };
    let extra = mag - base;
    for (i, &prob) in probs.iter().enumerate() {
        let shift = probs.len() - i - 1;
        w.write_bool(prob, extra & (1 << shift) != 0);
    }
}

pub(crate) const fn token_for_magnitude(mag: u16) -> u8 {
    match mag {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5..=6 => 5,
        7..=10 => 6,
        11..=18 => 7,
        19..=34 => 8,
        35..=66 => 9,
        67..=2114 => 10,
        _ => panic!("coefficient magnitude exceeds the VP8 token range"),
    }
}

fn mag_context(mag: u16) -> usize {
    match mag {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

pub(crate) struct ProbabilityFit {
    pub(crate) probs: [u8; 1056],
    pub(crate) savings: u64,
}

pub(crate) fn fit_probs(counts: &[[u32; 2]; 1056]) -> Option<ProbabilityFit> {
    let mut probs = DEFAULT_COEFF_PROBS;
    let mut savings = 0;
    for (i, counts) in counts.iter().enumerate() {
        let total = u64::from(counts[0]) + u64::from(counts[1]);
        if total == 0 {
            continue;
        }
        let old = probs[i];
        let old_cost = u64::from(counts[0]) * u64::from(bool_cost(old, false))
            + u64::from(counts[1]) * u64::from(bool_cost(old, true));
        let update_cost = u64::from(bool_cost(COEFF_UPDATE_PROBS[i], true)) + 8 * 256
            - u64::from(bool_cost(COEFF_UPDATE_PROBS[i], false));
        if old_cost <= update_cost {
            continue;
        }
        let fitted = ((u64::from(counts[0]) * 256 + total / 2) / total).clamp(1, 255) as u8;
        let new_cost = u64::from(counts[0]) * u64::from(bool_cost(fitted, false))
            + u64::from(counts[1]) * u64::from(bool_cost(fitted, true));
        let saving = old_cost
            .saturating_sub(new_cost)
            .saturating_sub(update_cost);
        if saving > 0 {
            probs[i] = fitted;
            savings += saving;
        }
    }
    (savings > 0).then_some(ProbabilityFit { probs, savings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macroblock::DenseMacroblock;

    fn emitted_block_cost(
        levels: &[i16; 16],
        plane: CoefficientPlane,
        ctx0: usize,
        probs: &[u8; 1056],
    ) -> u64 {
        let len = last_nonzero(levels, plane.first_position()).map_or(0, |last| last + 1);
        let prefix: [i16; 16] = std::array::from_fn(|pos| levels[ZIGZAG[pos]]);
        let mut w = CostWriter::new();
        write_block(&mut w, &prefix[..len], plane, ctx0, probs);
        w.bits()
    }

    fn emitted_token_cost(
        plane: CoefficientPlane,
        pos: usize,
        ctx: usize,
        prev_zero: bool,
        mag: Option<u16>,
        probs: &[u8; 1056],
    ) -> u64 {
        let mut w = CostWriter::new();
        let tok = mag.map_or(TOKEN_END, token_for_magnitude);
        write_token(&mut w, tok, (plane.index(), pos, ctx), prev_zero, probs);
        if let Some(mag) = mag.filter(|&m| m != 0) {
            write_extra_bits(&mut w, mag, tok);
            w.write_bool(128, false);
        }
        w.bits()
    }

    fn prob_cases() -> [[u8; 1056]; 2] {
        [
            DEFAULT_COEFF_PROBS,
            std::array::from_fn(|i| (1 + i * 71 % 255) as u8),
        ]
    }

    const PLANES: [CoefficientPlane; 4] = [
        CoefficientPlane::LumaAc,
        CoefficientPlane::LumaDc,
        CoefficientPlane::Chroma,
        CoefficientPlane::Luma,
    ];

    const TOKEN_MAGNITUDES: [u16; 20] = [
        0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 18, 19, 34, 35, 66, 67, 127, 128, 129, 2047,
    ];

    const CACHED_MAGNITUDES: [u16; 19] = [
        0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 15, 16, 18, 19, 34, 35, 66, 67, 2047,
    ];

    type CostRow = (String, u64, u64);

    fn assert_cost_rows(rows: Vec<CostRow>) {
        for (subject, estimated, emitted) in rows {
            assert_eq!(estimated, emitted, "{subject}");
        }
    }

    #[test]
    fn token_tree_paths() {
        for probs in prob_cases() {
            let mut w = BoolWriter::new();
            let mut ref_w = BoolWriter::new();
            let mut counts = TokenCounts::new();
            let mut ref_counts = Box::new([[0; 2]; 1056]);
            for plane in 0..4 {
                for (pos, &band) in COEFF_BANDS.iter().enumerate() {
                    for ctx in 0..3 {
                        for start in [0, 2] {
                            for tok in 0..12 {
                                let mut path = [false; 8];
                                let Some(len) = tree_path(&COEFF_TREE, start, tok, &mut path)
                                else {
                                    continue;
                                };
                                write_token(&mut w, tok, (plane, pos, ctx), start != 0, &probs);
                                write_token(
                                    &mut counts,
                                    tok,
                                    (plane, pos, ctx),
                                    start != 0,
                                    &probs,
                                );
                                let entry = (plane * 8 + band) * 3 + ctx;
                                let mut node = start;
                                for branch in path.into_iter().take(len) {
                                    ref_w.write_bool(probs[entry * 11 + node / 2], branch);
                                    ref_counts[entry * 11 + node / 2][usize::from(branch)] += 1;
                                    node = COEFF_TREE[node + usize::from(branch)] as usize;
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(w.finish(), ref_w.finish());
            assert_eq!(counts.into_counts(), ref_counts);
        }
    }

    #[test]
    fn cost_model_exact() {
        for probs in prob_cases() {
            let costs = TokenCosts::new(&probs);
            assert_cost_rows(token_cost_rows(&costs, &probs));
            assert_cost_rows(block_cost_rows(&costs, &probs));
            assert_cost_rows(macroblock_cost_rows(probs));
        }
    }

    fn token_cost_rows(costs: &TokenCosts, probs: &[u8; 1056]) -> Vec<CostRow> {
        let mut rows = Vec::new();
        for plane in PLANES {
            for ctx in 0..3 {
                for pos in plane.first_position()..16 {
                    for prev_zero in [false, true] {
                        for mag in TOKEN_MAGNITUDES {
                            rows.push((
                                format!(
                                    "token {} {pos}/{ctx} prev_zero={prev_zero} mag={mag}",
                                    plane.index()
                                ),
                                u64::from(costs.coefficient(plane, pos, ctx, prev_zero, mag)),
                                emitted_token_cost(plane, pos, ctx, prev_zero, Some(mag), probs),
                            ));
                        }
                    }
                }
            }
        }
        for plane in PLANES {
            for group in 0..8 {
                let pos = plane.first_position() + 2 * group;
                if pos + 1 >= 16 {
                    continue;
                }
                for ctx in 0..3 {
                    let pair_ctx = (plane.index() * 8 + group) * 3 + ctx;
                    for (packed, &rate) in costs.coeff_pairs[pair_ctx].iter().enumerate() {
                        let m0 = (packed & (PAIR_MAGNITUDES - 1)) as u16;
                        let m1 = (packed >> PAIR_MAGNITUDES.trailing_zeros()) as u16;
                        rows.push((
                            format!("pair {} {pos}/{ctx} {m0}+{m1}", plane.index()),
                            u64::from(rate),
                            emitted_token_cost(
                                plane,
                                pos,
                                ctx,
                                group != 0 && ctx == 0,
                                Some(m0),
                                probs,
                            ) + emitted_token_cost(
                                plane,
                                pos + 1,
                                mag_context(m0),
                                m0 == 0,
                                Some(m1),
                                probs,
                            ),
                        ));
                    }
                }
            }
        }
        rows
    }

    fn block_cost_rows(costs: &TokenCosts, probs: &[u8; 1056]) -> Vec<CostRow> {
        let mut rows = Vec::new();
        for plane in PLANES {
            for ctx in 0..3 {
                for last in plane.first_position()..16 {
                    for seed in 0..32usize {
                        let mut levels = [0i16; 16];
                        for pos in plane.first_position()..last {
                            levels[ZIGZAG[pos]] = ((seed * 31 + pos * 7) % 8) as i16;
                        }
                        levels[ZIGZAG[last]] = ((seed * 17 + last) % 7 + 1) as i16;
                        rows.push((
                            format!("block-low {} {last}/{ctx} seed={seed}", plane.index()),
                            costs.block(&levels, plane, ctx),
                            emitted_block_cost(&levels, plane, ctx, probs),
                        ));
                    }
                }
                for seed in 0..64usize {
                    let mags = [0, 1, -1, 2, -2, 15, -15, 16, -16, 127, -128, 2047];
                    let levels =
                        std::array::from_fn(|pos| mags[(seed * 73 + pos * 31) % mags.len()]);
                    rows.push((
                        format!("block-dense {}/{ctx} seed={seed}", plane.index()),
                        costs.block(&levels, plane, ctx),
                        emitted_block_cost(&levels, plane, ctx, probs),
                    ));
                }
                for pos in plane.first_position()..16 {
                    for mag in CACHED_MAGNITUDES {
                        let mut levels = [0; 16];
                        levels[ZIGZAG[pos]] = -(mag as i16);
                        rows.push((
                            format!("block-cached {} {pos}/{ctx} {mag}", plane.index()),
                            costs.block(&levels, plane, ctx),
                            emitted_block_cost(&levels, plane, ctx, probs),
                        ));
                    }
                }
            }
        }
        rows
    }

    fn zero_mb() -> DenseMacroblock {
        DenseMacroblock {
            luma: Luma::Intra16 { mode: 0 },
            y2: [0; 16],
            y: [[0; 16]; 16],
            chroma_mode: 0,
            u: [[0; 16]; 4],
            v: [[0; 16]; 4],
        }
    }

    fn dense_mb(round: usize) -> DenseMacroblock {
        let (luma, y2) = if round.is_multiple_of(3) {
            (Luma::Intra4 { modes: [0; 16] }, [0; 16])
        } else {
            let mut dc = [0; 16];
            dc[0] = 7;
            (Luma::Intra16 { mode: 0 }, dc)
        };
        let mut mb = DenseMacroblock {
            luma,
            y2,
            ..zero_mb()
        };
        for (i, blk) in mb.y.iter_mut().enumerate() {
            blk[ZIGZAG[(i + round) % 16]] = (i * round) as i16;
            if matches!(&mb.luma, Luma::Intra16 { .. }) {
                blk[0] = 0;
            }
        }
        mb.u[round % 4][15] = [-128, 127, -129, 128][round % 4];
        mb.v[(round + 1) % 4][4] = 1;
        mb
    }

    fn pack(dense: DenseMacroblock) -> (Macroblock, CoefficientSpool) {
        let mut spool = CoefficientSpool::new();
        let mb = Macroblock::pack(dense, &mut spool);
        (mb, spool)
    }

    fn macroblock_cost_rows(probs: [u8; 1056]) -> Vec<CostRow> {
        let costs = TokenCosts::new(&probs);
        let mut tokens = TokenWriter::with_probs(2, probs);
        let mut rows = Vec::new();
        for round in 0..8 {
            let col = round % 2;
            let dense = dense_mb(round);
            let (mb, _spool) = pack(dense);
            let ctx = tokens.contexts.contexts(col);
            let (empty_y, empty_uv) = ctx.empty_rates(&costs);
            let y_only = DenseMacroblock {
                luma: dense.luma,
                y2: dense.y2,
                y: dense.y,
                ..zero_mb()
            };
            let uv_only = DenseMacroblock {
                u: dense.u,
                v: dense.v,
                ..zero_mb()
            };
            let y2 = match &dense.luma {
                Luma::Intra16 { .. } => Some(&dense.y2),
                Luma::Intra4 { .. } => None,
            };
            let mut y_ctx = ctx;
            let y_rate = y_ctx.luma_cost(&costs, y2, &dense.y);
            let mut uv_ctx = ctx;
            let uv_rate = uv_ctx.chroma_cost(&costs, &dense.u, &dense.v);
            for (label, cand, rate) in [
                ("dense", &dense, y_rate + uv_rate),
                ("luma", &y_only, y_rate + empty_uv),
                ("chroma", &uv_only, empty_y + uv_rate),
            ] {
                let (cand, cand_spool) = pack(*cand);
                let mut w = CostWriter::new();
                tokens
                    .clone()
                    .write_macroblock(&mut w, col, &cand, &cand_spool);
                rows.push((format!("mb {label} round {round}"), rate, w.bits()));
            }
            tokens.contexts.advance(col, &mb);
        }
        rows
    }

    #[test]
    fn prob_fit_saves_bits() {
        assert!(fit_probs(&[[0; 2]; 1056]).is_none());
        let mut counts = [[0; 2]; 1056];
        counts[0] = [16_777_216, 0];
        counts[1] = [u32::MAX, u32::MAX / 3];
        counts[2] = [0, 32];
        counts[3] = [0, 1];
        let fitted = fit_probs(&counts).expect("the test counts repay their updates");
        assert_eq!(fitted.probs[0], 255);
        assert_eq!(fitted.probs[1], 192);
        assert_eq!(fitted.probs[2], 1);
        assert_eq!(fitted.probs[3], DEFAULT_COEFF_PROBS[3]);
        assert!(fitted.savings > 0);
    }

    #[test]
    fn zigzag_permutation() {
        let mut seen = [false; 16];
        for pos in ZIGZAG {
            assert!(!seen[pos]);
            seen[pos] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }
}
