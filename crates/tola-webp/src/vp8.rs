//! VP8 key-frame header, bool-coded frame header, and macroblock mode coding.

use crate::bool_writer::{BoolWriter, tree_paths};
use crate::cost::bool_cost;
use crate::tables::{COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS};
use crate::token::TokenSink;

const KF_Y_MODE_TREE: [i8; 8] = [-4, 2, 4, 6, 0, -1, -2, -3];
const KF_Y_MODE_PROBS: [u8; 4] = [145, 156, 163, 128];

const UV_MODE_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
const KF_UV_MODE_PROBS: [u8; 3] = [142, 114, 183];

pub(crate) const FIRST_PARTITION_LIMIT: usize = 1 << 19;

/// Write the bool-coded key-frame header (RFC 6386 §19.2).
pub(crate) fn write_frame_header(
    w: &mut BoolWriter,
    q: u8,
    lf: u8,
    probs: &[u8; 1056],
    skip_prob: Option<u8>,
) {
    w.write_literal(0, 1); // color_space: BT.601
    w.write_literal(0, 1); // clamping_type: none
    w.write_literal(0, 1); // segmentation_enabled
    w.write_literal(0, 1); // filter_type: normal
    w.write_literal(u32::from(lf.min(63)), 6);
    w.write_literal(0, 3); // sharpness_level
    w.write_literal(0, 1); // mode_ref_lf_delta_enabled
    w.write_literal(0, 2); // log2_nbr_of_dct_partitions: 1
    w.write_literal(u32::from(q.min(127)), 7);
    for _ in 0..5 {
        w.write_literal(0, 1); // zero qindex deltas
    }
    w.write_literal(1, 1); // refresh_entropy_probs
    for (i, &prob) in COEFF_UPDATE_PROBS.iter().enumerate() {
        if probs[i] == DEFAULT_COEFF_PROBS[i] {
            w.write_bool(prob, false);
        } else {
            w.write_bool(prob, true);
            w.write_literal(u32::from(probs[i]), 8);
        }
    }
    w.write_bool(128, skip_prob.is_some());
    if let Some(prob) = skip_prob {
        w.write_literal(u32::from(prob), 8);
    }
}

const BMODE_TREE: [i8; 18] = [
    0, 2, -1, 4, -2, 6, 8, 12, -3, 10, -5, -6, -4, 14, -7, 16, -8, -9,
];

const Y_MODE_PATHS: [(u8, u8); 5] = tree_paths(&KF_Y_MODE_TREE, 0);
const UV_MODE_PATHS: [(u8, u8); 4] = tree_paths(&UV_MODE_TREE, 0);
const BMODE_PATHS: [(u8, u8); 10] = tree_paths(&BMODE_TREE, 0);

#[inline]
fn write_path<W: TokenSink>(w: &mut W, tree: &[i8], probs: &[u8], paths: &[(u8, u8)], val: u8) {
    let (bits, len) = paths[usize::from(val)];
    let mut node = 0;
    for shift in (0..len).rev() {
        let branch = (bits >> shift) & 1 != 0;
        w.write_bool(probs[node >> 1], branch);
        node = tree[node + usize::from(branch)] as usize;
    }
}

static BMODE_COSTS: [[u32; 10]; 100] = {
    let mut costs = [[0; 10]; 100];
    let mut ctx = 0;
    while ctx < costs.len() {
        tree_costs(
            &BMODE_TREE,
            &crate::tables::KF_BMODE_PROBS,
            ctx * 9,
            0,
            0,
            &mut costs[ctx],
        );
        ctx += 1;
    }
    costs
};

const LUMA_MODE_COSTS: [u32; 5] = {
    let mut costs = [0; 5];
    tree_costs(&KF_Y_MODE_TREE, &KF_Y_MODE_PROBS, 0, 0, 0, &mut costs);
    costs
};

const CHROMA_MODE_COSTS: [u32; 4] = {
    let mut costs = [0; 4];
    tree_costs(&UV_MODE_TREE, &KF_UV_MODE_PROBS, 0, 0, 0, &mut costs);
    costs
};

const fn tree_costs<const N: usize>(
    tree: &[i8],
    probs: &[u8],
    start: usize,
    node: usize,
    cost: u32,
    costs: &mut [u32; N],
) {
    let mut branch = 0;
    while branch < 2 {
        let child = tree[node + branch];
        let next_cost = cost + bool_cost(probs[start + node / 2], branch != 0);
        if child <= 0 {
            costs[(-child) as usize] = next_cost;
        } else {
            tree_costs(tree, probs, start, child as usize, next_cost, costs);
        }
        branch += 1;
    }
}

const fn i16_to_bmode(mode: u8) -> u8 {
    match mode {
        1 => 2, // V_PRED -> B_VE_PRED
        2 => 3, // H_PRED -> B_HE_PRED
        3 => 1, // TM_PRED -> B_TM_PRED
        _ => 0, // DC_PRED -> B_DC_PRED
    }
}

pub(crate) fn i16_mode_cost(luma: u8) -> u64 {
    u64::from(LUMA_MODE_COSTS[usize::from(luma)])
}

pub(crate) fn chroma_mode_cost(mode: u8) -> u64 {
    u64::from(CHROMA_MODE_COSTS[usize::from(mode)])
}

#[derive(Clone)]
pub(crate) struct BmodeContexts {
    above: Vec<[u8; 4]>,
    left: [u8; 4],
}

impl BmodeContexts {
    pub(crate) fn new(mb_cols: usize) -> Self {
        Self {
            above: vec![[0; 4]; mb_cols],
            left: [0; 4],
        }
    }

    pub(crate) fn write_intra<W: TokenSink>(
        &mut self,
        w: &mut W,
        mb_x: usize,
        luma: u8,
        chroma: u8,
    ) {
        write_path(w, &KF_Y_MODE_TREE, &KF_Y_MODE_PROBS, &Y_MODE_PATHS, luma);
        write_path(w, &UV_MODE_TREE, &KF_UV_MODE_PROBS, &UV_MODE_PATHS, chroma);
        self.advance_intra(mb_x, luma);
    }

    pub(crate) fn write_b_pred<W: TokenSink>(
        &mut self,
        w: &mut W,
        mb_x: usize,
        modes: &[u8; 16],
        chroma: u8,
    ) {
        if mb_x == 0 {
            self.left = [0; 4];
        }
        write_path(w, &KF_Y_MODE_TREE, &KF_Y_MODE_PROBS, &Y_MODE_PATHS, 4);
        for block in 0..16 {
            let (above, left) = self.pair(mb_x, block, modes);
            let start = (usize::from(above) * 10 + usize::from(left)) * 9;
            write_path(
                w,
                &BMODE_TREE,
                &crate::tables::KF_BMODE_PROBS[start..start + 9],
                &BMODE_PATHS,
                modes[block],
            );
        }
        write_path(w, &UV_MODE_TREE, &KF_UV_MODE_PROBS, &UV_MODE_PATHS, chroma);
        self.set_modes(mb_x, *modes);
    }

    pub(crate) fn pair(&self, mb_x: usize, block: usize, modes: &[u8; 16]) -> (u8, u8) {
        let bx = block % 4;
        let by = block / 4;
        let above = if by == 0 {
            self.above[mb_x][bx]
        } else {
            modes[(by - 1) * 4 + bx]
        };
        let left = if bx == 0 {
            if mb_x == 0 { 0 } else { self.left[by] }
        } else {
            modes[by * 4 + bx - 1]
        };
        (above, left)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn left_modes(&self) -> [u8; 4] {
        self.left
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_left_modes(&mut self, left: [u8; 4]) {
        self.left = left;
    }

    pub(crate) fn advance_intra(&mut self, mb_x: usize, luma: u8) {
        if mb_x == 0 {
            self.left = [0; 4];
        }
        self.set_modes(mb_x, [i16_to_bmode(luma); 16]);
    }

    pub(crate) fn advance_b_pred(&mut self, mb_x: usize, modes: &[u8; 16]) {
        if mb_x == 0 {
            self.left = [0; 4];
        }
        self.set_modes(mb_x, *modes);
    }

    pub(crate) fn mode_cost(&self, mb_x: usize, block: usize, modes: &[u8; 16], mode: u8) -> u64 {
        let (above, left) = self.pair(mb_x, block, modes);
        u64::from(BMODE_COSTS[usize::from(above) * 10 + usize::from(left)][usize::from(mode)])
    }

    pub(crate) fn i4_mode_cost(&self, mb_x: usize, modes: &[u8; 16]) -> u64 {
        u64::from(LUMA_MODE_COSTS[4])
            + modes
                .iter()
                .enumerate()
                .map(|(block, &mode)| self.mode_cost(mb_x, block, modes, mode))
                .sum::<u64>()
    }

    fn set_modes(&mut self, mb_x: usize, modes: [u8; 16]) {
        self.above[mb_x] = [modes[12], modes[13], modes[14], modes[15]];
        self.left = [modes[3], modes[7], modes[11], modes[15]];
    }
}

pub(crate) struct Frame {
    hdr: [u8; 10],
    first: Vec<u8>,
    res: Vec<u8>,
}

impl Frame {
    pub(crate) const HEADER_BYTES: usize = 10;

    pub(crate) fn new(w: u16, h: u16, first: Vec<u8>, res: Vec<u8>) -> Option<Self> {
        if first.len() >= FIRST_PARTITION_LIMIT {
            return None;
        }
        let tag = ((first.len() as u32) << 5) | (1 << 4);
        let mut hdr = [0; Self::HEADER_BYTES];
        hdr[..3].copy_from_slice(&tag.to_le_bytes()[..3]);
        hdr[3..6].copy_from_slice(&[0x9d, 0x01, 0x2a]);
        hdr[6..8].copy_from_slice(&w.to_le_bytes());
        hdr[8..].copy_from_slice(&h.to_le_bytes());
        Some(Self { hdr, first, res })
    }

    pub(crate) fn chunks(&self) -> [&[u8]; 3] {
        [&self.hdr, &self.first, &self.res]
    }

    pub(crate) fn len(&self) -> usize {
        Self::HEADER_BYTES + self.first.len() + self.res.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bool_writer::tree_path;
    use crate::cost::CostWriter;

    #[test]
    fn header_partition_limit() {
        let size = FIRST_PARTITION_LIMIT - 1;
        let frame = Frame::new(16, 16, vec![0; size], Vec::new()).unwrap();
        let [hdr, _, _] = frame.chunks();
        let tag = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], 0]);
        assert_eq!(tag >> 5, size as u32);
        assert!(Frame::new(16, 16, vec![0; size + 1], Vec::new()).is_none());
    }

    #[test]
    fn bmode_costs_match_bits() {
        for above in 0..10 {
            for left in 0..10 {
                let ctx = above * 10 + left;
                for mode in 0..10 {
                    let mut w = CostWriter::new();
                    let mut path = [false; 8];
                    let len = tree_path(&BMODE_TREE, 0, mode, &mut path).unwrap();
                    let mut node = 0;
                    for &branch in &path[..len] {
                        w.write_bool(crate::tables::KF_BMODE_PROBS[ctx * 9 + node / 2], branch);
                        node = BMODE_TREE[node + usize::from(branch)] as usize;
                    }
                    assert_eq!(
                        u64::from(BMODE_COSTS[ctx][usize::from(mode)]),
                        w.bits(),
                        "above {above}, left {left}, mode {mode}"
                    );
                }
            }
        }
    }
}
