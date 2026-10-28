//! One VP8 key frame: the uncompressed header, the bool-coded frame header, and macroblock modes.

use crate::bool_writer::{BoolWriter, tree_paths};
use crate::cost::bool_cost;
use crate::tables::COEFF_UPDATE_PROBS;
use crate::tables::DEFAULT_COEFF_PROBS;
use crate::token::TokenSink;

/// Key-frame luma intra-mode tree (RFC 6386 §11.2).
const KF_Y_MODE_TREE: [i8; 8] = [-4, 2, 4, 6, 0, -1, -2, -3];
/// Branch probabilities of [`KF_Y_MODE_TREE`].
const KF_Y_MODE_PROBS: [u8; 4] = [145, 156, 163, 128];

/// Chroma intra-mode tree shared by U and V (RFC 6386 §11.4).
const UV_MODE_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
/// Branch probabilities of [`UV_MODE_TREE`].
const KF_UV_MODE_PROBS: [u8; 3] = [142, 114, 183];

/// Write the bool-coded key-frame header (RFC 6386 §19.2).
pub(crate) fn write_frame_header(
    writer: &mut BoolWriter,
    quantizer: u8,
    filter_level: u8,
    updates: &[u8; 1056],
) {
    writer.write_literal(0, 1); // color_space: BT.601
    writer.write_literal(0, 1); // clamping_type: no clamping
    writer.write_literal(0, 1); // segmentation_enabled
    writer.write_literal(0, 1); // filter_type: normal
    writer.write_literal(u32::from(filter_level.min(63)), 6); // loop_filter_level
    writer.write_literal(0, 3); // sharpness_level
    writer.write_literal(0, 1); // mode_ref_lf_delta_enabled
    writer.write_literal(0, 2); // log2_nbr_of_dct_partitions: one partition
    writer.write_literal(u32::from(quantizer.min(127)), 7); // base_qindex
    for _ in 0..5 {
        writer.write_literal(0, 1); // no delta on any quantizer index
    }
    writer.write_literal(1, 1); // refresh_entropy_probs
    for (index, probability) in COEFF_UPDATE_PROBS.iter().enumerate() {
        if updates[index] == DEFAULT_COEFF_PROBS[index] {
            writer.write_bool(*probability, false);
        } else {
            writer.write_bool(*probability, true);
            writer.write_literal(u32::from(updates[index]), 8);
        }
    }
    writer.write_literal(0, 1); // mb_no_skip_coeff: every macroblock carries tokens
}

/// RFC 6386 §11.3: the 4×4 luma subblock mode tree.
const BMODE_TREE: [i8; 18] = [
    0, 2, -1, 4, -2, 6, 8, 12, -3, 10, -5, -6, -4, 14, -7, 16, -8, -9,
];

// Packed branch paths per leaf. A tree's topology is fixed, so only the probability slice
// passed to `write_path` varies with the coding context.
const Y_MODE_PATHS: [(u8, u8); 5] = tree_paths(&KF_Y_MODE_TREE, 0);
const UV_MODE_PATHS: [(u8, u8); 4] = tree_paths(&UV_MODE_TREE, 0);
const BMODE_PATHS: [(u8, u8); 10] = tree_paths(&BMODE_TREE, 0);

/// Write one leaf of a VP8 tree using its precomputed branch path.
#[inline]
fn write_path<W: TokenSink>(
    writer: &mut W,
    tree: &[i8],
    probabilities: &[u8],
    paths: &[(u8, u8)],
    value: u8,
) {
    let (bits, length) = paths[usize::from(value)];
    let mut node = 0;
    for shift in (0..length).rev() {
        let branch = (bits >> shift) & 1 != 0;
        writer.write_bool(probabilities[node >> 1], branch);
        node = tree[node + usize::from(branch)] as usize;
    }
}

static BMODE_COSTS: [[u32; 10]; 100] = {
    let mut costs = [[0; 10]; 100];
    let mut context = 0;
    while context < costs.len() {
        tree_costs(
            &BMODE_TREE,
            &crate::tables::KF_BMODE_PROBS,
            context * 9,
            0,
            0,
            &mut costs[context],
        );
        context += 1;
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
    probabilities: &[u8],
    start: usize,
    node: usize,
    cost: u32,
    costs: &mut [u32; N],
) {
    let mut branch = 0;
    while branch < 2 {
        let child = tree[node + branch];
        let cost = cost + bool_cost(probabilities[start + node / 2], branch != 0);
        if child <= 0 {
            costs[(-child) as usize] = cost;
        } else {
            tree_costs(tree, probabilities, start, child as usize, cost, costs);
        }
        branch += 1;
    }
}

/// The luma header cost of an intra-16×16 macroblock, in 1/256 bit.
pub(crate) fn i16_mode_cost(luma: u8) -> u64 {
    u64::from(LUMA_MODE_COSTS[usize::from(luma)])
}

/// The shared U/V header cost, in 1/256 bit.
pub(crate) fn chroma_mode_cost(mode: u8) -> u64 {
    u64::from(CHROMA_MODE_COSTS[usize::from(mode)])
}

/// The subblock modes already coded above and to the left of the current macroblock.
#[derive(Clone)]
pub(crate) struct BmodeContexts {
    above: Vec<[u8; 4]>,
    left: [u8; 4],
}

impl BmodeContexts {
    pub(crate) fn new(macroblock_columns: usize) -> Self {
        Self {
            above: vec![[0; 4]; macroblock_columns],
            left: [0; 4],
        }
    }

    /// Write an intra-16×16 macroblock's luma and chroma modes.
    pub(crate) fn write_intra<W: TokenSink>(
        &mut self,
        writer: &mut W,
        macroblock_x: usize,
        luma: u8,
        chroma: u8,
    ) {
        if macroblock_x == 0 {
            self.left = [0; 4];
        }
        write_path(
            writer,
            &KF_Y_MODE_TREE,
            &KF_Y_MODE_PROBS,
            &Y_MODE_PATHS,
            luma,
        );
        write_path(
            writer,
            &UV_MODE_TREE,
            &KF_UV_MODE_PROBS,
            &UV_MODE_PATHS,
            chroma,
        );
        let derived = match luma {
            1 => 2, // V_PRED → B_VE_PRED
            2 => 3, // H_PRED → B_HE_PRED
            3 => 1, // TM_PRED → B_TM_PRED
            _ => 0, // DC_PRED → B_DC_PRED
        };
        self.set_modes(macroblock_x, [derived; 16]);
    }

    /// Write a B_PRED macroblock's 16 subblock modes and its chroma mode.
    pub(crate) fn write_b_pred<W: TokenSink>(
        &mut self,
        writer: &mut W,
        macroblock_x: usize,
        modes: &[u8; 16],
        chroma: u8,
    ) {
        if macroblock_x == 0 {
            self.left = [0; 4];
        }
        write_path(writer, &KF_Y_MODE_TREE, &KF_Y_MODE_PROBS, &Y_MODE_PATHS, 4);
        for block in 0..16 {
            let block_x = block % 4;
            let block_y = block / 4;
            let above = if block_y == 0 {
                self.above[macroblock_x][block_x]
            } else {
                modes[(block_y - 1) * 4 + block_x]
            };
            let left = if block_x == 0 {
                self.left[block_y]
            } else {
                modes[block_y * 4 + block_x - 1]
            };
            let start = (usize::from(above) * 10 + usize::from(left)) * 9;
            write_path(
                writer,
                &BMODE_TREE,
                &crate::tables::KF_BMODE_PROBS[start..start + 9],
                &BMODE_PATHS,
                modes[block],
            );
        }
        write_path(
            writer,
            &UV_MODE_TREE,
            &KF_UV_MODE_PROBS,
            &UV_MODE_PATHS,
            chroma,
        );
        self.set_modes(macroblock_x, *modes);
    }

    /// The above and left subblock modes for one block of a B_PRED macroblock.
    pub(crate) fn pair(&self, macroblock_x: usize, block: usize, modes: &[u8; 16]) -> (u8, u8) {
        let block_x = block % 4;
        let block_y = block / 4;
        let above = if block_y == 0 {
            self.above[macroblock_x][block_x]
        } else {
            modes[(block_y - 1) * 4 + block_x]
        };
        let left = if block_x == 0 {
            if macroblock_x == 0 {
                0
            } else {
                self.left[block_y]
            }
        } else {
            modes[block_y * 4 + block_x - 1]
        };
        (above, left)
    }

    /// The four subblock modes on the left edge of the next macroblock.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn left_modes(&self) -> [u8; 4] {
        self.left
    }

    /// Replace the left modes with the previous strip's final macroblock of this row.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_left_modes(&mut self, left: [u8; 4]) {
        self.left = left;
    }

    /// Advance the subblock contexts after an intra-16×16 macroblock.
    pub(crate) fn advance_intra(&mut self, macroblock_x: usize, luma: u8) {
        if macroblock_x == 0 {
            self.left = [0; 4];
        }
        let derived = match luma {
            1 => 2,
            2 => 3,
            3 => 1,
            _ => 0,
        };
        self.set_modes(macroblock_x, [derived; 16]);
    }

    /// Advance the subblock contexts after a B_PRED macroblock.
    pub(crate) fn advance_b_pred(&mut self, macroblock_x: usize, modes: &[u8; 16]) {
        if macroblock_x == 0 {
            self.left = [0; 4];
        }
        self.set_modes(macroblock_x, *modes);
    }

    /// The cost of coding one subblock mode under the current contexts, in 1/256 bit.
    pub(crate) fn mode_cost(
        &self,
        macroblock_x: usize,
        block: usize,
        modes: &[u8; 16],
        mode: u8,
    ) -> u64 {
        let (above, left) = self.pair(macroblock_x, block, modes);
        u64::from(BMODE_COSTS[usize::from(above) * 10 + usize::from(left)][usize::from(mode)])
    }

    /// The luma header cost of a B_PRED macroblock, in 1/256 bit.
    pub(crate) fn i4_mode_cost(&self, macroblock_x: usize, modes: &[u8; 16]) -> u64 {
        u64::from(LUMA_MODE_COSTS[4])
            + modes
                .iter()
                .enumerate()
                .map(|(block, &mode)| self.mode_cost(macroblock_x, block, modes, mode))
                .sum::<u64>()
    }

    fn set_modes(&mut self, macroblock_x: usize, modes: [u8; 16]) {
        self.above[macroblock_x] = [modes[12], modes[13], modes[14], modes[15]];
        self.left = [modes[3], modes[7], modes[11], modes[15]];
    }
}

/// Assemble the VP8 frame header and its partitions (RFC 6386 §9.1).
pub(crate) fn assemble_frame(
    width: u16,
    height: u16,
    first: &[u8],
    residual: &[u8],
) -> Option<Vec<u8>> {
    if first.len() >= 1 << 19 {
        return None;
    }
    let first_size = first.len() as u32;

    let mut out = Vec::with_capacity(10 + first.len() + residual.len());
    // Key frame, version 0, show frame, then the first partition's size.
    let tag = (first_size << 5) | (1 << 4);
    out.extend_from_slice(&tag.to_le_bytes()[..3]);
    out.extend_from_slice(&[0x9d, 0x01, 0x2a]);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(first);
    out.extend_from_slice(residual);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::CostWriter;

    #[test]
    fn frame_headers_respect_size_limit() {
        let header = vec![0; (1 << 19) - 1];
        let frame = assemble_frame(16, 16, &header, &[]).unwrap();
        let tag = u32::from_le_bytes([frame[0], frame[1], frame[2], 0]);
        assert_eq!(tag >> 5, header.len() as u32);
        assert!(assemble_frame(16, 16, &vec![0; 1 << 19], &[]).is_none());
    }

    #[test]
    fn subblock_costs_match_coded_modes() {
        for above in 0..10 {
            for left in 0..10 {
                let context = above * 10 + left;
                for mode in 0..10 {
                    let mut writer = CostWriter::new();
                    writer.write_tree_from(
                        &BMODE_TREE,
                        &crate::tables::KF_BMODE_PROBS[context * 9..context * 9 + 9],
                        mode,
                        0,
                    );
                    assert_eq!(
                        u64::from(BMODE_COSTS[context][usize::from(mode)]),
                        writer.bits(),
                        "above {above}, left {left}, mode {mode}"
                    );
                }
            }
        }
    }
}
