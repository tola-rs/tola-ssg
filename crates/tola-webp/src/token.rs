//! VP8 coefficient tokens: the scan, contexts, token tree, and extra bits (RFC 6386 §13).

use crate::bool_writer::{BoolWriter, LengthWriter, tree_path};
use crate::cost::CostWriter;
use crate::macroblock::{CoefficientSpool, Luma, Macroblock};
use crate::tables::{COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS};
use std::sync::{Arc, LazyLock};

/// RFC 6386 §13.2: the coefficient token tree.
/// Coefficient positions per block (4x4).
pub(crate) const BLOCK_COEFFICIENTS: usize = 16;
/// Probability contexts for a coefficient band.
pub(crate) const CONTEXTS: usize = 3;
/// Probability bands over the scan.
pub(crate) const BANDS: usize = 8;
/// Token categories per coefficient.
pub(crate) const TOKENS: usize = 12;
/// Coarse probability entries: plane * band * context.
pub(crate) const ENTRIES: usize = 96;
/// Probabilities per entry in the VP8 coefficient table.
pub(crate) const PROBABILITIES_PER_ENTRY: usize = 11;
/// Total coefficient probabilities in a VP8 frame.
pub(crate) const COEFFICIENT_PROBABILITIES: usize = ENTRIES * PROBABILITIES_PER_ENTRY;
/// Largest coefficient magnitude the token tables cover.
pub(crate) const MAGNITUDES: usize = 2048;
/// Largest magnitude served by the small-coefficient table.
pub(crate) const SMALL_MAGNITUDES: usize = 128;
/// Largest magnitude the six-bit pair table covers.
pub(crate) const PAIR_MAGNITUDES: usize = 64;
/// Packed pair-table entries (two six-bit magnitudes).
pub(crate) const PAIR_ENTRIES: usize = PAIR_MAGNITUDES * PAIR_MAGNITUDES;
/// Rows in the pair table: the 96 entries rounded up so the row mask is a power of two.
const PAIR_ROWS: usize = ENTRIES.next_power_of_two();

const COEFF_TREE: [i8; 22] = [
    -11, 2, 0, 4, -1, 6, 8, 12, -2, 10, -3, -4, 14, 16, -5, -6, 18, 20, -7, -8, -9, -10,
];

/// One coefficient-tree branch: probability offset within the entry, then the branch value.
type TokenStep = (u8, bool);

/// The fixed tree walk for one coefficient token, shared by branch counting and emission.
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

/// RFC 6386 §13.3: coefficient position to probability band.
pub(crate) const COEFF_BANDS: [usize; BLOCK_COEFFICIENTS] =
    [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7];

/// Probability entry for each plane and scan position, before adding the context.
const ENTRY_BASE: [[u16; BLOCK_COEFFICIENTS]; 4] = {
    let mut table = [[0u16; 16]; 4];
    let mut plane = 0;
    while plane < 4 {
        let mut position = 0;
        while position < 16 {
            table[plane][position] = ((plane * 8 + COEFF_BANDS[position]) * 3) as u16;
            position += 1;
        }
        plane += 1;
    }
    table
};

/// The block's coefficients in scan order, written back to raster positions.
pub(crate) const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Raster offsets of each scan pair packed four bits each, for planes whose first
/// coefficient is position 0 or 1.
const PAIR_SCAN: [[u8; 8]; 2] = [
    [
        1 << 4,
        4 | 8 << 4,
        5 | 2 << 4,
        3 | 6 << 4,
        9 | 12 << 4,
        13 | 10 << 4,
        7 | 11 << 4,
        14 | 15 << 4,
    ],
    [
        1 | 4 << 4,
        8 | 5 << 4,
        2 | 3 << 4,
        6 | 9 << 4,
        12 | 13 << 4,
        10 | 7 << 4,
        11 | 14 << 4,
        0,
    ],
];

/// RFC 6386 §13.2: extra-bit probabilities for each magnitude category.
const CATEGORY_ONE: [u8; 1] = [159];
const CATEGORY_TWO: [u8; 2] = [165, 145];
const CATEGORY_THREE: [u8; 3] = [173, 148, 140];
const CATEGORY_FOUR: [u8; 4] = [176, 155, 140, 135];
const CATEGORY_FIVE: [u8; 5] = [180, 157, 141, 134, 130];
const CATEGORY_SIX: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

/// Plane indices of the coefficient probability table.
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
        // DCT energy is four times pixel energy; the Y2 WHT adds another factor of four.
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
    magnitudes: &'static [u16; MAGNITUDES],
    small_coefficients: Arc<[[u16; SMALL_MAGNITUDES]; ENTRIES]>,
    // Masked to 127 in the hot loop, so every lookup is bounds-check free.
    coefficient_pairs6: Arc<[[u16; PAIR_ENTRIES]; PAIR_ROWS]>,
}

static MAGNITUDE_COSTS: LazyLock<[u16; MAGNITUDES]> = LazyLock::new(|| {
    std::array::from_fn(|magnitude| {
        if magnitude == 0 {
            return 0;
        }
        let mut writer = CostWriter::new();
        let token = token_for_magnitude(magnitude as u16);
        write_extra_bits(&mut writer, magnitude as u16, token);
        writer.write_bool(128, false);
        writer.bits() as u16
    })
});

static DEFAULT_TOKEN_COSTS: LazyLock<TokenCosts> =
    LazyLock::new(|| TokenCosts::new(&DEFAULT_COEFF_PROBS));

impl Default for TokenCosts {
    fn default() -> Self {
        DEFAULT_TOKEN_COSTS.clone()
    }
}

impl TokenCosts {
    pub(crate) fn new(probs: &[u8; COEFFICIENT_PROBABILITIES]) -> Self {
        let mut tokens = [[0; TOKENS]; ENTRIES];
        let mut roots = [0; ENTRIES];
        let magnitudes = &*MAGNITUDE_COSTS;
        for entry in 0..96 {
            let probabilities =
                &probs[entry * PROBABILITIES_PER_ENTRY..(entry + 1) * PROBABILITIES_PER_ENTRY];
            roots[entry] = crate::cost::bool_cost(probabilities[0], true) as u16;
            for token in 0..TOKENS as u8 {
                let mut writer = CostWriter::new();
                writer.write_tree_from(&COEFF_TREE, probabilities, token, 0);
                tokens[entry][usize::from(token)] = writer.bits() as u16;
            }
        }
        let small_coefficients: Arc<[[u16; SMALL_MAGNITUDES]; ENTRIES]> =
            Arc::new(std::array::from_fn(|entry| {
                std::array::from_fn(|magnitude| {
                    // (8 token branches + 11 extra bits) * 2048 + 256 sign units fits u16.
                    tokens[entry][usize::from(token_for_magnitude(magnitude as u16))]
                        + magnitudes[magnitude]
                })
            }));
        let table: Box<[[u16; PAIR_ENTRIES]; PAIR_ROWS]> = (0..PAIR_ROWS)
            .map(|entry| {
                if entry >= 96 {
                    return [0; 4096];
                }
                let context = entry % CONTEXTS;
                let group = entry / CONTEXTS % BANDS;
                let plane = entry / (CONTEXTS * BANDS);
                let position = usize::from(plane == PLANE_Y_AFTER_Y2) + 2 * group;
                std::array::from_fn(|packed| {
                    if position + 1 >= 16 {
                        return 0;
                    }
                    let first = packed & (PAIR_MAGNITUDES - 1);
                    let second = packed >> PAIR_MAGNITUDES.trailing_zeros();
                    let first_entry = (plane * 8 + COEFF_BANDS[position]) * 3 + context;
                    let second_entry = (plane * 8 + COEFF_BANDS[position + 1]) * 3 + first.min(2);
                    let first_cost = small_coefficients[first_entry][first]
                        - if group != 0 && context == 0 {
                            roots[first_entry]
                        } else {
                            0
                        };
                    let second_cost = small_coefficients[second_entry][second]
                        - if first == 0 { roots[second_entry] } else { 0 };
                    first_cost + second_cost
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .try_into()
            .expect("the pair table has 128 entries");
        let coefficient_pairs6 = Arc::from(table);
        Self {
            tokens,
            roots,
            magnitudes,
            small_coefficients,
            coefficient_pairs6,
        }
    }

    #[inline(always)]
    pub(crate) fn coefficient(
        &self,
        plane: CoefficientPlane,
        position: usize,
        context: usize,
        previous_was_zero: bool,
        magnitude: u16,
    ) -> u32 {
        if magnitude < SMALL_MAGNITUDES as u16 {
            let entry = (plane.index() * 8 + COEFF_BANDS[position]) * 3 + context;
            return u32::from(self.small_coefficients[entry][usize::from(magnitude)])
                - if previous_was_zero {
                    u32::from(self.roots[entry])
                } else {
                    0
                };
        }
        self.token(
            plane,
            position,
            context,
            previous_was_zero,
            token_for_magnitude(magnitude),
        ) + self.magnitude_cost(magnitude)
    }

    pub(crate) fn root(&self, plane: CoefficientPlane, position: usize, context: usize) -> u32 {
        let entry = (plane.index() * 8 + COEFF_BANDS[position]) * 3 + context;
        u32::from(self.roots[entry])
    }

    pub(crate) fn token(
        &self,
        plane: CoefficientPlane,
        position: usize,
        context: usize,
        previous_was_zero: bool,
        token: u8,
    ) -> u32 {
        let entry = (plane.index() * 8 + COEFF_BANDS[position]) * 3 + context;
        u32::from(self.tokens[entry][usize::from(token)])
            - if previous_was_zero {
                u32::from(self.roots[entry])
            } else {
                0
            }
    }

    pub(crate) fn magnitude_cost(&self, magnitude: u16) -> u32 {
        u32::from(self.magnitudes[usize::from(magnitude)])
    }

    #[inline(always)]
    pub(crate) fn end(&self, plane: CoefficientPlane, position: usize, context: usize) -> u32 {
        let entry = (plane.index() * 8 + COEFF_BANDS[position]) * 3 + context;
        u32::from(self.tokens[entry][usize::from(TOKEN_END)])
    }

    pub(crate) fn block(
        &self,
        levels: &[i16; 16],
        plane: CoefficientPlane,
        first_context: usize,
    ) -> u64 {
        self.block_rate(levels, plane, first_context).0
    }

    fn block_rate(
        &self,
        levels: &[i16; 16],
        plane: CoefficientPlane,
        first_context: usize,
    ) -> (u64, bool) {
        let first = plane.first_position();
        let Some(last) = last_nonzero(levels, first) else {
            return (u64::from(self.end(plane, first, first_context)), false);
        };
        let scan = &PAIR_SCAN[first & 1];
        let mut pair_entry = plane.index() * (CONTEXTS * BANDS);
        let mut rate = 0;
        let mut context = first_context;
        let mut position = first;
        // `last <= 15` bounds the walk to the eight fixed scan pairs.
        for &offsets in scan {
            if position >= last {
                break;
            }
            let first_magnitude = levels[usize::from(offsets & 15)].unsigned_abs();
            let second_magnitude = levels[usize::from(offsets >> 4)].unsigned_abs();
            if (first_magnitude | second_magnitude) >= 64 {
                return self.block_rate_wide(levels, plane, first, rate, position, context, last);
            }
            let packed = usize::from(first_magnitude | (second_magnitude << 6)) & 0xfff;
            rate += u64::from(
                self.coefficient_pairs6[(pair_entry + context) & (PAIR_ROWS - 1)][packed],
            );
            pair_entry += CONTEXTS;
            position += 2;
            context = usize::from(second_magnitude.min(2));
        }
        if position == last {
            let magnitude = levels[ZIGZAG[position]].unsigned_abs();
            let previous_was_zero = position != first && levels[ZIGZAG[position - 1]] == 0;
            if magnitude < SMALL_MAGNITUDES as u16 {
                let entry = (plane.index() * 8 + COEFF_BANDS[position]) * 3 + context;
                rate += u64::from(self.small_coefficients[entry][usize::from(magnitude)])
                    - if previous_was_zero {
                        u64::from(self.roots[entry])
                    } else {
                        0
                    };
            } else {
                rate += u64::from(self.coefficient(
                    plane,
                    position,
                    context,
                    previous_was_zero,
                    magnitude,
                ));
            }
            context = magnitude_context(magnitude);
        }
        if last < 15 {
            rate += u64::from(self.end(plane, last + 1, context));
        }
        (rate, true)
    }

    /// Continue from the first pair carrying a magnitude of 64 or more.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn block_rate_wide(
        &self,
        levels: &[i16; 16],
        plane: CoefficientPlane,
        first: usize,
        mut rate: u64,
        mut position: usize,
        mut context: usize,
        last: usize,
    ) -> (u64, bool) {
        let mut previous_was_zero = position != first && levels[ZIGZAG[position - 1]] == 0;
        while position < last {
            let first_magnitude = levels[ZIGZAG[position]].unsigned_abs();
            let second_magnitude = levels[ZIGZAG[position + 1]].unsigned_abs();
            rate += u64::from(self.coefficient(
                plane,
                position,
                context,
                previous_was_zero,
                first_magnitude,
            ));
            rate += u64::from(self.coefficient(
                plane,
                position + 1,
                magnitude_context(first_magnitude),
                first_magnitude == 0,
                second_magnitude,
            ));
            previous_was_zero = second_magnitude == 0;
            context = magnitude_context(second_magnitude);
            position += 2;
        }
        if position == last {
            let magnitude = levels[ZIGZAG[position]].unsigned_abs();
            rate +=
                u64::from(self.coefficient(plane, position, context, previous_was_zero, magnitude));
            context = magnitude_context(magnitude);
        }
        if last < 15 {
            rate += u64::from(self.end(plane, last + 1, context));
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
    pub(crate) fn y2(&self) -> usize {
        usize::from(self.above.y2) + usize::from(self.left.y2)
    }

    pub(crate) fn set_y2(&mut self, nonzero: bool) {
        self.above.y2 = nonzero;
        self.left.y2 = nonzero;
    }

    pub(crate) fn y(&self, index: usize) -> usize {
        usize::from(self.above.y[index % 4]) + usize::from(self.left.y[index / 4])
    }

    pub(crate) fn set_y(&mut self, index: usize, nonzero: bool) {
        self.above.y[index % 4] = nonzero;
        self.left.y[index / 4] = nonzero;
    }

    pub(crate) fn u(&self, index: usize) -> usize {
        usize::from(self.above.u[index % 2]) + usize::from(self.left.u[index / 2])
    }

    pub(crate) fn set_u(&mut self, index: usize, nonzero: bool) {
        self.above.u[index % 2] = nonzero;
        self.left.u[index / 2] = nonzero;
    }

    pub(crate) fn v(&self, index: usize) -> usize {
        usize::from(self.above.v[index % 2]) + usize::from(self.left.v[index / 2])
    }

    pub(crate) fn set_v(&mut self, index: usize, nonzero: bool) {
        self.above.v[index % 2] = nonzero;
        self.left.v[index / 2] = nonzero;
    }

    pub(crate) fn luma_cost(
        &mut self,
        costs: &TokenCosts,
        y2: Option<&[i16; 16]>,
        y: &[[i16; 16]; 16],
    ) -> u64 {
        let mut rate = 0;
        let plane = if let Some(y2) = y2 {
            let (block_rate, nonzero) = costs.block_rate(y2, CoefficientPlane::LumaDc, self.y2());
            rate += block_rate;
            self.set_y2(nonzero);
            CoefficientPlane::LumaAc
        } else {
            CoefficientPlane::Luma
        };
        for (index, block) in y.iter().enumerate() {
            let (block_rate, nonzero) = costs.block_rate(block, plane, self.y(index));
            rate += block_rate;
            self.set_y(index, nonzero);
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
        for (index, block) in u.iter().enumerate() {
            let (block_rate, nonzero) =
                costs.block_rate(block, CoefficientPlane::Chroma, self.u(index));
            rate += block_rate;
            self.set_u(index, nonzero);
        }
        for (index, block) in v.iter().enumerate() {
            let (block_rate, nonzero) =
                costs.block_rate(block, CoefficientPlane::Chroma, self.v(index));
            rate += block_rate;
            self.set_v(index, nonzero);
        }
        rate
    }
}

/// Coefficient branches carry probability indices; fixed extra bits and signs do not.
pub(crate) trait TokenSink {
    fn write_bool(&mut self, probability: u8, value: bool);
    #[inline(always)]
    fn coefficient_branch(&mut self, _index: usize, probability: u8, value: bool) {
        self.write_bool(probability, value);
    }
    fn write_tree_from(&mut self, tree: &[i8], probabilities: &[u8], value: u8, start: usize);
    /// Whether this sink only counts coefficient branches and can skip fixed extra bits.
    fn counts_only(&self) -> bool {
        false
    }
    /// Emits or accounts for one coefficient token; the default walks its compiled path.
    #[inline(always)]
    fn coefficient_token(
        &mut self,
        entry: usize,
        token: u8,
        skip_end_branch: bool,
        probabilities: &[u8; 1056],
    ) where
        Self: Sized,
    {
        let path = &COEFF_PATHS[usize::from(skip_end_branch)][usize::from(token)];
        write_path(self, path, entry * 11, probabilities);
    }
}

impl TokenSink for BoolWriter {
    #[inline(always)]
    fn write_bool(&mut self, probability: u8, value: bool) {
        BoolWriter::write_bool(self, probability, value);
    }

    fn write_tree_from(&mut self, tree: &[i8], probabilities: &[u8], value: u8, start: usize) {
        BoolWriter::write_tree_from(self, tree, probabilities, value, start);
    }
}

impl TokenSink for CostWriter {
    fn write_bool(&mut self, probability: u8, value: bool) {
        CostWriter::write_bool(self, probability, value);
    }

    fn write_tree_from(&mut self, tree: &[i8], probabilities: &[u8], value: u8, start: usize) {
        let mut path = [false; 8];
        let len = tree_path(tree, start, value, &mut path).expect("the value must be a tree leaf");
        let mut node = start;
        for branch in path.into_iter().take(len) {
            CostWriter::write_bool(self, probabilities[node >> 1], branch);
            node = tree[node + usize::from(branch)] as usize;
        }
    }
}

const TOKEN_END: u8 = 11;

/// Branch outcomes per coefficient-probability entry: `[zero, one]`.
pub(crate) type BranchCounts = Box<[[u32; 2]; 1056]>;

/// How often each coefficient token occurs, keyed by probability entry and by whether a
/// preceding zero coefficient skipped the leading EOB branch.
pub(crate) struct TokenCounts {
    tokens: Box<[[u32; 16]; 256]>,
}

impl TokenCounts {
    pub(crate) fn new() -> Self {
        Self {
            tokens: Box::new([[0; 16]; 256]),
        }
    }

    /// Expands the token histogram into the branch counts the fit consumes. One expansion per
    /// frame costs 96 x 12 path walks instead of one walk per emitted token.
    pub(crate) fn into_counts(self) -> BranchCounts {
        let mut counts = Box::new([[0; 2]; 1056]);
        for (slot, histogram) in self.tokens[..192].iter().enumerate() {
            let entry = slot / 2;
            let skip_end_branch = slot % 2;
            for (token, &occurrences) in histogram.iter().enumerate() {
                if occurrences == 0 {
                    continue;
                }
                for &(offset, branch) in COEFF_PATHS[skip_end_branch][token].branches() {
                    counts[entry * 11 + usize::from(offset)][usize::from(branch)] += occurrences;
                }
            }
        }
        counts
    }
}

impl TokenSink for TokenCounts {
    fn write_bool(&mut self, _probability: u8, _value: bool) {}

    fn counts_only(&self) -> bool {
        true
    }

    fn coefficient_token(
        &mut self,
        entry: usize,
        token: u8,
        skip_end_branch: bool,
        _probabilities: &[u8; 1056],
    ) {
        // Masked power-of-two indices keep both dimensions provably in bounds.
        self.tokens[(entry * 2 + usize::from(skip_end_branch)) & 255][usize::from(token) & 15] += 1;
    }

    fn write_tree_from(&mut self, _tree: &[i8], _probabilities: &[u8], _value: u8, _start: usize) {}
}

impl TokenSink for LengthWriter {
    fn write_bool(&mut self, probability: u8, value: bool) {
        LengthWriter::write_bool(self, probability, value);
    }

    fn write_tree_from(&mut self, tree: &[i8], probabilities: &[u8], value: u8, start: usize) {
        let mut path = [false; 8];
        let len = tree_path(tree, start, value, &mut path).expect("the value must be a tree leaf");
        let mut node = start;
        for branch in path.into_iter().take(len) {
            LengthWriter::write_bool(self, probabilities[node >> 1], branch);
            node = tree[node + usize::from(branch)] as usize;
        }
    }
}

/// Emits the fitted residual partition while a length-only state tracks the default table.
///
/// The token writer must use the fitted table, so each coefficient branch's `probability` is
/// `fitted_probs[index]`; the length state substitutes `DEFAULT_COEFF_PROBS[index]`. Fixed
/// extra-bit and sign probabilities are identical under both tables.
pub(crate) struct FittedPartition<'a> {
    writer: BoolWriter,
    default_length: LengthWriter,
    fitted_probs: &'a [u8; 1056],
}

impl<'a> FittedPartition<'a> {
    pub(crate) fn new(fitted_probs: &'a [u8; 1056]) -> Self {
        Self {
            writer: BoolWriter::new(),
            default_length: LengthWriter::new(),
            fitted_probs,
        }
    }

    /// The fitted partition bytes and the default partition's exact length.
    pub(crate) fn finish(self) -> (Vec<u8>, usize) {
        (self.writer.finish(), self.default_length.finish())
    }
}

impl TokenSink for FittedPartition<'_> {
    fn write_bool(&mut self, probability: u8, value: bool) {
        self.writer.write_bool(probability, value);
        self.default_length.write_bool(probability, value);
    }

    fn coefficient_branch(&mut self, index: usize, probability: u8, value: bool) {
        debug_assert_eq!(probability, self.fitted_probs[index]);
        self.writer.write_bool(probability, value);
        self.default_length
            .write_bool(DEFAULT_COEFF_PROBS[index], value);
    }

    fn write_tree_from(&mut self, tree: &[i8], probabilities: &[u8], value: u8, start: usize) {
        self.writer
            .write_tree_from(tree, probabilities, value, start);
        self.default_length
            .write_tree_from(tree, probabilities, value, start);
    }
}

#[derive(Clone)]
pub(crate) struct TokenContexts {
    above: Vec<Edges>,
    left: Edges,
}

impl TokenContexts {
    pub(crate) fn new(macroblock_columns: usize) -> Self {
        Self {
            above: vec![Edges::default(); macroblock_columns],
            left: Edges::default(),
        }
    }

    pub(crate) fn contexts(&self, macroblock_x: usize) -> MacroblockContexts {
        MacroblockContexts {
            above: self.above[macroblock_x],
            left: if macroblock_x == 0 {
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

    /// Replace the left edge with the previous strip's final macroblock of this row.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_left(&mut self, left: Edges) {
        self.left = left;
    }

    pub(crate) fn advance(&mut self, macroblock_x: usize, macroblock: &Macroblock) {
        let mut neighbors = self.contexts(macroblock_x);
        let first = match &macroblock.luma {
            Luma::Intra16 { dc, .. } => {
                neighbors.set_y2(dc.iter().any(|level| *level != 0));
                1
            }
            Luma::Intra4 { .. } => 0,
        };
        for (index, &length) in macroblock.lengths[..16].iter().enumerate() {
            neighbors.set_y(index, usize::from(length) > first);
        }
        for (index, &length) in macroblock.lengths[16..20].iter().enumerate() {
            neighbors.set_u(index, length != 0);
        }
        for (index, &length) in macroblock.lengths[20..].iter().enumerate() {
            neighbors.set_v(index, length != 0);
        }
        self.commit(macroblock_x, neighbors);
    }

    fn commit(&mut self, macroblock_x: usize, neighbors: MacroblockContexts) {
        self.above[macroblock_x] = neighbors.above;
        self.left = neighbors.left;
    }
}

#[derive(Clone)]
pub(crate) struct TokenWriter {
    probs: [u8; 1056],
    contexts: TokenContexts,
}

impl TokenWriter {
    pub(crate) fn new(macroblock_columns: usize) -> Self {
        Self::with_probs(macroblock_columns, DEFAULT_COEFF_PROBS)
    }

    pub(crate) fn with_probs(macroblock_columns: usize, probs: [u8; 1056]) -> Self {
        Self {
            probs,
            contexts: TokenContexts::new(macroblock_columns),
        }
    }

    #[inline(always)]
    pub(crate) fn write_macroblock<W: TokenSink>(
        &mut self,
        writer: &mut W,
        macroblock_x: usize,
        macroblock: &Macroblock,
        spool: &CoefficientSpool,
    ) {
        let mut neighbors = self.contexts.contexts(macroblock_x);
        let (plane, first_position) = match &macroblock.luma {
            Luma::Intra4 { .. } => (PLANE_Y_WITHOUT_Y2, 0),
            Luma::Intra16 { dc, .. } => {
                let (prefix, length) = scan_prefix(dc);
                let nonzero = write_block(
                    writer,
                    &prefix[..length],
                    PLANE_Y2,
                    0,
                    neighbors.y2(),
                    &self.probs,
                );
                neighbors.set_y2(nonzero);
                (PLANE_Y_AFTER_Y2, 1)
            }
        };
        let mut start = macroblock.offset as usize;
        for (index, &length) in macroblock.lengths[..16].iter().enumerate() {
            let block = spool.block(start, length);
            let nonzero = write_block(
                writer,
                block,
                plane,
                first_position,
                neighbors.y(index),
                &self.probs,
            );
            neighbors.set_y(index, nonzero);
            start += usize::from(length);
        }
        for (base, above, left) in [
            (16, &mut neighbors.above.u, &mut neighbors.left.u),
            (20, &mut neighbors.above.v, &mut neighbors.left.v),
        ] {
            for (index, &length) in macroblock.lengths[base..base + 4].iter().enumerate() {
                let column = index % 2;
                let row = index / 2;
                let context = usize::from(above[column]) + usize::from(left[row]);
                let block = spool.block(start, length);
                let nonzero = write_block(writer, block, PLANE_CHROMA, 0, context, &self.probs);
                above[column] = nonzero;
                left[row] = nonzero;
                start += usize::from(length);
            }
        }
        self.contexts.commit(macroblock_x, neighbors);
    }
}

/// Highest scan position at or above `first` (0 or 1) that carries a non-zero coefficient.
///
/// Descending groups let the common short block skip most of a linear fifteen-step scan.
#[inline]
fn last_nonzero(levels: &[i16; 16], first: usize) -> Option<usize> {
    let at = |position: usize| levels[ZIGZAG[position]];
    if at(8) | at(9) | at(10) | at(11) | at(12) | at(13) | at(14) | at(15) != 0 {
        return (8..16).rev().find(|&position| at(position) != 0);
    }
    if at(4) | at(5) | at(6) | at(7) != 0 {
        return (4..8).rev().find(|&position| at(position) != 0);
    }
    if at(2) | at(3) != 0 {
        return Some(if at(3) != 0 { 3 } else { 2 });
    }
    if at(1) != 0 {
        return Some(1);
    }
    (first == 0 && at(0) != 0).then_some(0)
}

/// The dense block's scan-order prefix and its length.
fn scan_prefix(levels: &[i16; 16]) -> ([i16; 16], usize) {
    let mut length = 0;
    for position in (0..16).rev() {
        if levels[ZIGZAG[position]] != 0 {
            length = position + 1;
            break;
        }
    }
    let prefix = std::array::from_fn(|position| levels[ZIGZAG[position]]);
    (prefix, length)
}

/// Write one block from its scan-order prefix and return whether it carries a coefficient at or
/// after `first_position`.
#[inline(always)]
fn write_block<W: TokenSink>(
    writer: &mut W,
    levels: &[i16],
    plane: usize,
    first_position: usize,
    first_context: usize,
    probs: &[u8; 1056],
) -> bool {
    if levels.len() <= first_position {
        write_token(
            writer,
            TOKEN_END,
            (plane, first_position, first_context),
            false,
            probs,
        );
        return false;
    }

    let last = levels.len() - 1;
    let mut context = first_context;
    let mut previous_was_zero = false;
    for (position, &level) in levels.iter().enumerate().skip(first_position) {
        let magnitude = level.unsigned_abs();
        let token = token_for_magnitude(magnitude);
        write_token(
            writer,
            token,
            (plane, position, context),
            previous_was_zero,
            probs,
        );
        if magnitude != 0 && !writer.counts_only() {
            write_extra_bits(writer, magnitude, token);
            writer.write_bool(128, level < 0);
        }
        context = magnitude_context(magnitude);
        previous_was_zero = magnitude == 0;
    }

    if last < 15 {
        write_token(writer, TOKEN_END, (plane, last + 1, context), false, probs);
    }
    true
}

/// Write one token's coefficient branches; `base` is the entry's first probability index.
#[inline(always)]
fn write_path<W: TokenSink>(writer: &mut W, path: &TokenPath, base: usize, probs: &[u8]) {
    for &(offset, branch) in path.branches() {
        let index = base + usize::from(offset);
        writer.coefficient_branch(index, probs[index], branch);
    }
}

#[inline(always)]
fn write_token<W: TokenSink>(
    writer: &mut W,
    token: u8,
    (plane, position, context): (usize, usize, usize),
    skip_end_branch: bool,
    probs: &[u8; 1056],
) {
    let entry = usize::from(ENTRY_BASE[plane][position]) + context;
    if !writer.counts_only() {
        assert_ne!(
            COEFF_PATHS[usize::from(skip_end_branch)][usize::from(token)].len,
            0,
            "the token must be a tree leaf"
        );
    }
    writer.coefficient_token(entry, token, skip_end_branch, probs);
}

fn write_extra_bits<W: TokenSink>(writer: &mut W, magnitude: u16, token: u8) {
    let (base, probabilities): (u16, &[u8]) = match token {
        5 => (5, &CATEGORY_ONE),
        6 => (7, &CATEGORY_TWO),
        7 => (11, &CATEGORY_THREE),
        8 => (19, &CATEGORY_FOUR),
        9 => (35, &CATEGORY_FIVE),
        10 => (67, &CATEGORY_SIX),
        _ => return,
    };
    let extra = magnitude - base;
    for (index, probability) in probabilities.iter().enumerate() {
        let shift = probabilities.len() - index - 1;
        writer.write_bool(*probability, extra & (1 << shift) != 0);
    }
}

pub(crate) fn token_for_level(level: u16) -> u8 {
    token_for_magnitude(level)
}

fn token_for_magnitude(magnitude: u16) -> u8 {
    match magnitude {
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
        _ => unreachable!("coefficient magnitude exceeds the VP8 token range"),
    }
}

fn magnitude_context(magnitude: u16) -> usize {
    match magnitude {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

#[cfg(test)]
pub(crate) fn emitted_block_cost(
    levels: &[i16; 16],
    plane: CoefficientPlane,
    first_context: usize,
    probs: &[u8; 1056],
) -> u64 {
    let length = last_nonzero(levels, plane.first_position()).map_or(0, |last| last + 1);
    let prefix: [i16; 16] = std::array::from_fn(|position| levels[ZIGZAG[position]]);
    let mut writer = CostWriter::new();
    write_block(
        &mut writer,
        &prefix[..length],
        plane.index(),
        plane.first_position(),
        first_context,
        probs,
    );
    writer.bits()
}

#[cfg(test)]
pub(crate) fn emitted_token_cost(
    plane: CoefficientPlane,
    position: usize,
    context: usize,
    previous_was_zero: bool,
    magnitude: Option<u16>,
    probs: &[u8; 1056],
) -> u64 {
    let mut writer = CostWriter::new();
    let token = magnitude.map_or(TOKEN_END, token_for_magnitude);
    write_token(
        &mut writer,
        token,
        (plane.index(), position, context),
        previous_was_zero,
        probs,
    );
    if let Some(magnitude) = magnitude.filter(|magnitude| *magnitude != 0) {
        write_extra_bits(&mut writer, magnitude, token);
        writer.write_bool(128, false);
    }
    writer.bits()
}

/// A fitted table with its estimated net savings in 1/256 bit.
pub(crate) struct ProbabilityFit {
    pub(crate) probs: [u8; 1056],
    pub(crate) savings: u64,
}

/// Empirical probabilities change only when estimated savings repay their header updates.
///
/// `None` means every entry stayed at its default. `savings` is the sum of net estimates over
/// changed entries; callers use it to decide whether exact acceptance is worth its cost.
pub(crate) fn fit_probs(counts: &[[u32; 2]; 1056]) -> Option<ProbabilityFit> {
    let mut probs = DEFAULT_COEFF_PROBS;
    let mut savings = 0;
    for (index, counts) in counts.iter().enumerate() {
        let total = u64::from(counts[0]) + u64::from(counts[1]);
        if total == 0 {
            continue;
        }
        let old = probs[index];
        let old_cost = u64::from(counts[0]) * u64::from(crate::cost::bool_cost(old, false))
            + u64::from(counts[1]) * u64::from(crate::cost::bool_cost(old, true));
        let update_cost = u64::from(crate::cost::bool_cost(COEFF_UPDATE_PROBS[index], true))
            + 8 * 256
            - u64::from(crate::cost::bool_cost(COEFF_UPDATE_PROBS[index], false));
        if old_cost <= update_cost {
            continue;
        }
        let fitted = ((u64::from(counts[0]) * 256 + total / 2) / total).clamp(1, 255) as u8;
        let new_cost = u64::from(counts[0]) * u64::from(crate::cost::bool_cost(fitted, false))
            + u64::from(counts[1]) * u64::from(crate::cost::bool_cost(fitted, true));
        let saving = old_cost
            .saturating_sub(new_cost)
            .saturating_sub(update_cost);
        if saving > 0 {
            probs[index] = fitted;
            savings += saving;
        }
    }
    (savings > 0).then_some(ProbabilityFit { probs, savings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macroblock::DenseMacroblock;

    fn coefficient_probability_cases() -> [[u8; 1056]; 2] {
        [DEFAULT_COEFF_PROBS, synthetic_probs()]
    }

    fn synthetic_probs() -> [u8; 1056] {
        std::array::from_fn(|index| (1 + index * 71 % 255) as u8)
    }

    const PLANES: [CoefficientPlane; 4] = [
        CoefficientPlane::LumaAc,
        CoefficientPlane::LumaDc,
        CoefficientPlane::Chroma,
        CoefficientPlane::Luma,
    ];

    /// Every magnitude a token can carry, including both sides of each band boundary.
    const TOKEN_MAGNITUDES: [u16; 20] = [
        0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 18, 19, 34, 35, 66, 67, 127, 128, 129, 2047,
    ];

    /// The magnitudes one coefficient at one position takes in the cached-rate check.
    const CACHED_MAGNITUDES: [u16; 19] = [
        0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 15, 16, 18, 19, 34, 35, 66, 67, 2047,
    ];

    /// One row of the cost-model contract: what the model estimates, what the writer emits for
    /// the same coefficients, and the case that names the disagreement when they differ.
    type CostRow = (String, u64, u64);

    fn assert_cost_rows(rows: Vec<CostRow>) {
        for (subject, estimated, emitted) in rows {
            assert_eq!(estimated, emitted, "{subject}");
        }
    }

    #[test]
    fn token_bits_match_tree_walk() {
        for probs in coefficient_probability_cases() {
            let mut writer = BoolWriter::new();
            let mut reference = BoolWriter::new();
            let mut counts = TokenCounts::new();
            let mut reference_counts = Box::new([[0; 2]; 1056]);
            for plane in 0..4 {
                for (position, &band) in COEFF_BANDS.iter().enumerate() {
                    for context in 0..3 {
                        for start in [0, 2] {
                            for token in 0..12 {
                                let mut path = [false; 8];
                                let Some(len) = tree_path(&COEFF_TREE, start, token, &mut path)
                                else {
                                    continue;
                                };
                                write_token(
                                    &mut writer,
                                    token,
                                    (plane, position, context),
                                    start != 0,
                                    &probs,
                                );
                                write_token(
                                    &mut counts,
                                    token,
                                    (plane, position, context),
                                    start != 0,
                                    &probs,
                                );
                                let entry = (plane * 8 + band) * 3 + context;
                                let mut node = start;
                                for branch in path.into_iter().take(len) {
                                    reference.write_bool(probs[entry * 11 + node / 2], branch);
                                    reference_counts[entry * 11 + node / 2][usize::from(branch)] +=
                                        1;
                                    node = COEFF_TREE[node + usize::from(branch)] as usize;
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(writer.finish(), reference.finish());
            assert_eq!(counts.into_counts(), reference_counts);
        }
    }

    #[test]
    fn scan_covers_every_coefficient() {
        let mut seen = [false; 16];
        for position in ZIGZAG {
            assert!(!seen[position]);
            seen[position] = true;
        }
        assert!(seen.iter().all(|seen| *seen));
    }

    /// Every rate the model computes equals the bits the writer emits for the same coefficients:
    /// one token, one cached pair, one block, one macroblock.
    ///
    /// The model exists only to make the encoder's searches cheap; the moment its arithmetic
    /// drifts from the writer's the encoder optimises a different file than it produces.
    #[test]
    fn cost_model_matches_emitted_bits() {
        for probs in coefficient_probability_cases() {
            let costs = TokenCosts::new(&probs);
            assert_cost_rows(token_cost_rows(&costs, &probs));
            assert_cost_rows(block_cost_rows(&costs, &probs));
            assert_cost_rows(macroblock_cost_rows(probs));
        }
    }

    fn token_cost_rows(costs: &TokenCosts, probs: &[u8; 1056]) -> Vec<CostRow> {
        let mut rows = Vec::new();
        for plane in PLANES {
            for context in 0..3 {
                for position in plane.first_position()..16 {
                    for previous_was_zero in [false, true] {
                        for magnitude in TOKEN_MAGNITUDES {
                            rows.push((
                                format!(
                                    "token {} {position}/{context} previous_was_zero={previous_was_zero} magnitude={magnitude}",
                                plane.index()
                                ),
                                u64::from(costs.coefficient(
                                    plane,
                                    position,
                                    context,
                                    previous_was_zero,
                                    magnitude,
                                )),
                                emitted_token_cost(
                                    plane,
                                    position,
                                    context,
                                    previous_was_zero,
                                    Some(magnitude),
                                    probs,
                                ),
                            ));
                        }
                    }
                }
            }
        }
        for plane in PLANES {
            for group in 0..8 {
                let position = plane.first_position() + 2 * group;
                if position + 1 >= 16 {
                    continue;
                }
                for context in 0..3 {
                    let pair_context = (plane.index() * 8 + group) * 3 + context;
                    for (packed, &rate) in costs.coefficient_pairs6[pair_context].iter().enumerate()
                    {
                        let first = (packed & 63) as u16;
                        let second = (packed >> 6) as u16;
                        rows.push((
                            format!(
                                "pair {} {position}/{context} {first}+{second}",
                                plane.index()
                            ),
                            u64::from(rate),
                            emitted_token_cost(
                                plane,
                                position,
                                context,
                                group != 0 && context == 0,
                                Some(first),
                                probs,
                            ) + emitted_token_cost(
                                plane,
                                position + 1,
                                magnitude_context(first),
                                first == 0,
                                Some(second),
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
            for context in 0..3 {
                for last in plane.first_position()..16 {
                    for seed in 0..32usize {
                        let mut levels = [0i16; 16];
                        for position in plane.first_position()..last {
                            levels[ZIGZAG[position]] = ((seed * 31 + position * 7) % 8) as i16;
                        }
                        levels[ZIGZAG[last]] = ((seed * 17 + last) % 7 + 1) as i16;
                        rows.push((
                            format!("block-low {} {last}/{context} seed={seed}", plane.index()),
                            costs.block(&levels, plane, context),
                            emitted_block_cost(&levels, plane, context, probs),
                        ));
                    }
                }
                for seed in 0..64usize {
                    let magnitudes = [0, 1, -1, 2, -2, 15, -15, 16, -16, 127, -128, 2047];
                    let levels = std::array::from_fn(|position| {
                        magnitudes[(seed * 73 + position * 31) % magnitudes.len()]
                    });
                    rows.push((
                        format!("block-dense {}/{context} seed={seed}", plane.index()),
                        costs.block(&levels, plane, context),
                        emitted_block_cost(&levels, plane, context, probs),
                    ));
                }
                for position in plane.first_position()..16 {
                    for magnitude in CACHED_MAGNITUDES {
                        let mut levels = [0; 16];
                        levels[ZIGZAG[position]] = -(magnitude as i16);
                        rows.push((
                            format!(
                                "block-cached {} {position}/{context} {magnitude}",
                                plane.index()
                            ),
                            costs.block(&levels, plane, context),
                            emitted_block_cost(&levels, plane, context, probs),
                        ));
                    }
                }
            }
        }
        rows
    }

    fn zero_macroblock() -> DenseMacroblock {
        DenseMacroblock {
            luma: Luma::Intra16 {
                mode: 0,
                dc: [0; 16],
            },
            y: [[0; 16]; 16],
            chroma_mode: 0,
            u: [[0; 16]; 4],
            v: [[0; 16]; 4],
        }
    }

    fn dense_macroblock(round: usize) -> DenseMacroblock {
        let luma = if round.is_multiple_of(3) {
            Luma::Intra4 { modes: [0; 16] }
        } else {
            let mut dc = [0; 16];
            dc[0] = 7;
            Luma::Intra16 { mode: 0, dc }
        };
        let mut macroblock = DenseMacroblock {
            luma,
            ..zero_macroblock()
        };
        for (index, block) in macroblock.y.iter_mut().enumerate() {
            block[ZIGZAG[(index + round) % 16]] = (index * round) as i16;
            if matches!(&macroblock.luma, Luma::Intra16 { .. }) {
                block[0] = 0;
            }
        }
        macroblock.u[round % 4][15] = -67;
        macroblock.v[(round + 1) % 4][4] = 1;
        macroblock
    }

    fn pack(dense: DenseMacroblock) -> (Macroblock, CoefficientSpool) {
        let mut spool = CoefficientSpool::new();
        let macroblock = Macroblock::pack(dense, &mut spool);
        (macroblock, spool)
    }

    fn macroblock_cost_rows(probs: [u8; 1056]) -> Vec<CostRow> {
        let costs = TokenCosts::new(&probs);
        let mut tokens = TokenWriter::with_probs(2, probs);
        let mut rows = Vec::new();
        for round in 0..8 {
            let column = round % 2;
            let dense = dense_macroblock(round);
            let (macroblock, _spool) = pack(dense);
            let neighbors = tokens.contexts.contexts(column);
            let mut empty_neighbors = neighbors;
            let zero_chroma = empty_neighbors.chroma_cost(&costs, &[[0; 16]; 4], &[[0; 16]; 4]);
            let zero_luma = empty_neighbors.luma_cost(&costs, Some(&[0; 16]), &[[0; 16]; 16]);
            let luma_only = DenseMacroblock {
                luma: dense.luma,
                y: dense.y,
                ..zero_macroblock()
            };
            let chroma_only = DenseMacroblock {
                u: dense.u,
                v: dense.v,
                ..zero_macroblock()
            };
            let luma_dc = match &dense.luma {
                Luma::Intra16 { dc, .. } => Some(dc),
                Luma::Intra4 { .. } => None,
            };
            let mut luma_neighbors = neighbors;
            let luma_rate = luma_neighbors.luma_cost(&costs, luma_dc, &dense.y);
            let mut chroma_neighbors = neighbors;
            let chroma_rate = chroma_neighbors.chroma_cost(&costs, &dense.u, &dense.v);
            for (label, candidate, rate) in [
                ("dense", &dense, luma_rate + chroma_rate),
                ("luma", &luma_only, luma_rate + zero_chroma),
                ("chroma", &chroma_only, zero_luma + chroma_rate),
            ] {
                let (candidate, candidate_spool) = pack(*candidate);
                let mut emitted = CostWriter::new();
                tokens
                    .clone()
                    .write_macroblock(&mut emitted, column, &candidate, &candidate_spool);
                rows.push((
                    format!("macroblock {label} round {round}"),
                    rate,
                    emitted.bits(),
                ));
            }
            tokens.contexts.advance(column, &macroblock);
        }
        rows
    }

    #[test]
    fn probability_updates_repay_header_cost() {
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
}
