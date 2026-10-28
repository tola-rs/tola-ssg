//! Intra prediction for 16×16 luma and 8×8 chroma blocks (RFC 6386 §12.2–12.3).

use crate::reconstruction::Plane;

/// The four intra modes shared by luma 16×16 and chroma blocks.
pub(crate) const DC: u8 = 0;
pub(crate) const VERTICAL: u8 = 1;
pub(crate) const HORIZONTAL: u8 = 2;
pub(crate) const TRUE_MOTION: u8 = 3;

/// Prediction candidates share neighbors until their macroblock is committed.
pub(crate) struct Edges<const SIDE: usize> {
    top: [u8; SIDE],
    left: [u8; SIDE],
    corner: u8,
    dc: u8,
}

impl Edges<16> {
    pub(crate) fn luma(plane: &Plane, macroblock_x: usize, macroblock_y: usize) -> Self {
        Self::from_plane(plane, macroblock_x, macroblock_y, 4, 5)
    }
}

impl Edges<8> {
    pub(crate) fn chroma(plane: &Plane, macroblock_x: usize, macroblock_y: usize) -> Self {
        Self::from_plane(plane, macroblock_x, macroblock_y, 3, 4)
    }
}

impl<const SIDE: usize> Edges<SIDE> {
    fn from_plane(
        plane: &Plane,
        macroblock_x: usize,
        macroblock_y: usize,
        edge_shift: u32,
        both_shift: u32,
    ) -> Self {
        let x = macroblock_x * SIDE;
        let y = macroblock_y * SIDE;
        let mut top = [127; SIDE];
        let mut left = [129; SIDE];
        let mut sum = 0u32;
        if macroblock_y > 0 {
            top.copy_from_slice(plane.slice(x, y - 1, SIDE));
            sum += top.iter().map(|sample| u32::from(*sample)).sum::<u32>();
        }
        if macroblock_x > 0 {
            for (row, sample) in left.iter_mut().enumerate() {
                *sample = plane.sample(x - 1, y + row);
                sum += u32::from(*sample);
            }
        }
        let dc = if macroblock_x == 0 && macroblock_y == 0 {
            128
        } else {
            let shift = if macroblock_x > 0 && macroblock_y > 0 {
                both_shift
            } else {
                edge_shift
            };
            ((sum + (1 << (shift - 1))) >> shift) as u8
        };
        let corner = if macroblock_y == 0 {
            127
        } else if macroblock_x == 0 {
            129
        } else {
            plane.sample(x - 1, y - 1)
        };
        Self {
            top,
            left,
            corner,
            dc,
        }
    }
}

/// Predicted 16×16 luma block for `mode`.
pub(crate) fn luma_block(mode: u8, edges: &Edges<16>) -> [u8; 256] {
    block::<256, 16>(mode, edges)
}

/// Predicted 8×8 chroma block for `mode`.
pub(crate) fn chroma_block(mode: u8, edges: &Edges<8>) -> [u8; 64] {
    block::<64, 8>(mode, edges)
}

/// VP8's ten 4×4 luma prediction modes.
pub(crate) const B_DC: u8 = 0;
pub(crate) const B_TM: u8 = 1;
pub(crate) const B_VE: u8 = 2;
pub(crate) const B_HE: u8 = 3;
pub(crate) const B_LD: u8 = 4;
pub(crate) const B_RD: u8 = 5;
pub(crate) const B_VR: u8 = 6;
pub(crate) const B_VL: u8 = 7;
pub(crate) const B_HD: u8 = 8;
pub(crate) const B_HU: u8 = 9;

fn avg2(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b) + 1) >> 1) as u8
}

fn avg3(a: u8, b: u8, c: u8) -> u8 {
    ((u16::from(a) + 2 * u16::from(b) + u16::from(c) + 2) >> 2) as u8
}

/// Shared 4×4 prediction edges: the 36 averages every ten-mode search reads.
pub(crate) struct Borders {
    top: [u8; 8],
    left: [u8; 4],
    corner: u8,
    dc: u8,
    top3: [u8; 8],
    left3: [u8; 4],
    edge3: [u8; 7],
    top2: [u8; 6],
    left2: [u8; 3],
    edge2: [u8; 8],
}

impl Borders {
    pub(crate) fn new(top: &[u8; 8], left: &[u8; 4], corner: u8) -> Self {
        let top_edge = [
            corner, top[0], top[1], top[2], top[3], top[4], top[5], top[6], top[7],
        ];
        let left_edge = [corner, left[0], left[1], left[2], left[3]];
        let edge = [
            left[3], left[2], left[1], left[0], corner, top[0], top[1], top[2], top[3],
        ];
        let mut top3 = [0; 8];
        for i in 0..7 {
            top3[i] = avg3(top_edge[i], top_edge[i + 1], top_edge[i + 2]);
        }
        top3[7] = avg3(top[6], top[7], top[7]);
        let mut left3 = [0; 4];
        for i in 0..3 {
            left3[i] = avg3(left_edge[i], left_edge[i + 1], left_edge[i + 2]);
        }
        left3[3] = avg3(left[2], left[3], left[3]);
        let mut edge3 = [0; 7];
        for i in 0..7 {
            edge3[i] = avg3(edge[i], edge[i + 1], edge[i + 2]);
        }
        let mut top2 = [0; 6];
        for i in 0..6 {
            top2[i] = avg2(top[i], top[i + 1]);
        }
        let mut left2 = [0; 3];
        for i in 0..3 {
            left2[i] = avg2(left[i], left[i + 1]);
        }
        let mut edge2 = [0; 8];
        for i in 0..8 {
            edge2[i] = avg2(edge[i], edge[i + 1]);
        }
        let sum: u32 = top[..4]
            .iter()
            .chain(left.iter())
            .map(|value| u32::from(*value))
            .sum();
        Self {
            top: *top,
            left: *left,
            corner,
            dc: ((sum + 4) >> 3) as u8,
            top3,
            left3,
            edge3,
            top2,
            left2,
            edge2,
        }
    }

    pub(crate) fn block(&self, mode: u8) -> [u8; 16] {
        let mut output = [0u8; 16];
        match mode {
            B_DC => output.fill(self.dc),
            B_TM => {
                for row in 0..4 {
                    for column in 0..4 {
                        output[row * 4 + column] = (i32::from(self.left[row])
                            + i32::from(self.top[column])
                            - i32::from(self.corner))
                        .clamp(0, 255) as u8;
                    }
                }
            }
            B_VE => {
                let values = [self.top3[0], self.top3[1], self.top3[2], self.top3[3]];
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].copy_from_slice(&values);
                }
            }
            B_HE => {
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].fill(self.left3[row]);
                }
            }
            B_LD => {
                let values = [
                    self.top3[1],
                    self.top3[2],
                    self.top3[3],
                    self.top3[4],
                    self.top3[5],
                    self.top3[6],
                    self.top3[7],
                ];
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].copy_from_slice(&values[row..row + 4]);
                }
            }
            B_RD => {
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].copy_from_slice(&self.edge3[3 - row..7 - row]);
                }
            }
            B_VR => {
                output[12] = self.edge3[1];
                output[8] = self.edge3[2];
                output[13] = self.edge3[3];
                output[4] = self.edge3[3];
                output[14] = self.edge3[4];
                output[5] = self.edge3[4];
                output[15] = self.edge3[5];
                output[6] = self.edge3[5];
                output[7] = self.edge3[6];
                output[9] = self.edge2[4];
                output[0] = self.edge2[4];
                output[10] = self.edge2[5];
                output[1] = self.edge2[5];
                output[11] = self.edge2[6];
                output[2] = self.edge2[6];
                output[3] = self.edge2[7];
            }
            B_VL => {
                output[0] = self.top2[0];
                output[4] = self.top3[1];
                output[8] = self.top2[1];
                output[1] = self.top2[1];
                output[5] = self.top3[2];
                output[12] = self.top3[2];
                output[9] = self.top2[2];
                output[2] = self.top2[2];
                output[13] = self.top3[3];
                output[6] = self.top3[3];
                output[10] = self.top2[3];
                output[3] = self.top2[3];
                output[14] = self.top3[4];
                output[7] = self.top3[4];
                output[11] = self.top3[5];
                output[15] = self.top3[6];
            }
            B_HD => {
                output[12] = self.edge2[0];
                output[13] = self.edge3[0];
                output[8] = self.edge2[1];
                output[14] = self.edge2[1];
                output[9] = self.edge3[1];
                output[15] = self.edge3[1];
                output[10] = self.edge2[2];
                output[4] = self.edge2[2];
                output[11] = self.edge3[2];
                output[5] = self.edge3[2];
                output[6] = self.edge2[3];
                output[0] = self.edge2[3];
                output[7] = self.edge3[3];
                output[1] = self.edge3[3];
                output[2] = self.edge3[4];
                output[3] = self.edge3[5];
            }
            B_HU => {
                output[0] = self.left2[0];
                output[1] = self.left3[1];
                output[2] = self.left2[1];
                output[4] = self.left2[1];
                output[3] = self.left3[2];
                output[5] = self.left3[2];
                output[6] = self.left2[2];
                output[8] = self.left2[2];
                output[7] = self.left3[3];
                output[9] = self.left3[3];
                output[10..].fill(self.left[3]);
            }
            _ => unreachable!("only the ten subblock modes are encoded"),
        }
        output
    }
}

fn block<const N: usize, const SIDE: usize>(mode: u8, edges: &Edges<SIDE>) -> [u8; N] {
    if mode == DC {
        return [edges.dc; N];
    }
    let mut output = [0u8; N];
    match mode {
        VERTICAL => {
            for row in 0..SIDE {
                output[row * SIDE..(row + 1) * SIDE].copy_from_slice(&edges.top);
            }
        }
        HORIZONTAL => {
            for (row, sample) in edges.left.iter().enumerate() {
                output[row * SIDE..(row + 1) * SIDE].fill(*sample);
            }
        }
        TRUE_MOTION => {
            for (row, left) in edges.left.iter().enumerate() {
                for (column, top) in edges.top.iter().enumerate() {
                    output[row * SIDE + column] = (i32::from(*left) + i32::from(*top)
                        - i32::from(edges.corner))
                    .clamp(0, 255) as u8;
                }
            }
        }
        _ => unreachable!("only the three directional modes reach this block"),
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without neighbours a mode falls back to the block's own default; with them it predicts
    /// from what is above and left.
    #[test]
    fn edges_predict_from_neighbours_or_defaults() {
        let mut plane = Plane::luma(32, 2);
        let corner = Edges::luma(&plane, 0, 0);
        assert_eq!(luma_block(DC, &corner), [128; 256]);
        assert_eq!(luma_block(VERTICAL, &corner), [127; 256]);
        assert_eq!(luma_block(HORIZONTAL, &corner), [129; 256]);
        assert_eq!(luma_block(TRUE_MOTION, &corner), [129; 256]);

        let borders = Borders::new(&[127; 8], &[129; 4], 127);
        assert_eq!(borders.block(B_DC), [128; 16]);
        assert_eq!(borders.block(B_TM), [129; 16]);
        assert_eq!(borders.block(B_VE), [127; 16]);
        assert_eq!(borders.block(B_HE), [129; 16]);

        // The same block once its neighbours exist: a corner between 100 above and 50 to the
        // left averages to 75, which is the DC the exhaustive table also expects.
        for column in 0..16 {
            plane.row_mut(15)[16 + column] = 100;
            plane.row_mut(16 + column)[15] = 50;
        }
        assert_eq!(luma_block(DC, &Edges::luma(&plane, 1, 1)), [75; 256]);
    }

    fn scalar_avg2(a: u8, b: u8) -> u8 {
        ((u32::from(a) + u32::from(b) + 1) >> 1) as u8
    }

    fn scalar_avg3(a: u8, b: u8, c: u8) -> u8 {
        ((u32::from(a) + 2 * u32::from(b) + u32::from(c) + 2) >> 2) as u8
    }

    // The pre-shared ten-mode equations, kept as the independent oracle.
    fn scalar_subblock(mode: u8, top: &[u8; 8], left: &[u8; 4], corner: u8) -> [u8; 16] {
        let edge = [
            left[3], left[2], left[1], left[0], corner, top[0], top[1], top[2], top[3],
        ];
        let mut output = [0u8; 16];
        match mode {
            B_DC => {
                let sum: u32 = top[..4]
                    .iter()
                    .chain(left.iter())
                    .map(|value| u32::from(*value))
                    .sum();
                output.fill(((sum + 4) >> 3) as u8);
            }
            B_TM => {
                for row in 0..4 {
                    for column in 0..4 {
                        output[row * 4 + column] = (i32::from(left[row]) + i32::from(top[column])
                            - i32::from(corner))
                        .clamp(0, 255) as u8;
                    }
                }
            }
            B_VE => {
                let values = [
                    scalar_avg3(corner, top[0], top[1]),
                    scalar_avg3(top[0], top[1], top[2]),
                    scalar_avg3(top[1], top[2], top[3]),
                    scalar_avg3(top[2], top[3], top[4]),
                ];
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].copy_from_slice(&values);
                }
            }
            B_HE => {
                let values = [
                    scalar_avg3(corner, left[0], left[1]),
                    scalar_avg3(left[0], left[1], left[2]),
                    scalar_avg3(left[1], left[2], left[3]),
                    scalar_avg3(left[2], left[3], left[3]),
                ];
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].fill(values[row]);
                }
            }
            B_LD => {
                let values = [
                    scalar_avg3(top[0], top[1], top[2]),
                    scalar_avg3(top[1], top[2], top[3]),
                    scalar_avg3(top[2], top[3], top[4]),
                    scalar_avg3(top[3], top[4], top[5]),
                    scalar_avg3(top[4], top[5], top[6]),
                    scalar_avg3(top[5], top[6], top[7]),
                    scalar_avg3(top[6], top[7], top[7]),
                ];
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].copy_from_slice(&values[row..row + 4]);
                }
            }
            B_RD => {
                let values = [
                    scalar_avg3(edge[0], edge[1], edge[2]),
                    scalar_avg3(edge[1], edge[2], edge[3]),
                    scalar_avg3(edge[2], edge[3], edge[4]),
                    scalar_avg3(edge[3], edge[4], edge[5]),
                    scalar_avg3(edge[4], edge[5], edge[6]),
                    scalar_avg3(edge[5], edge[6], edge[7]),
                    scalar_avg3(edge[6], edge[7], edge[8]),
                ];
                for row in 0..4 {
                    output[row * 4..(row + 1) * 4].copy_from_slice(&values[3 - row..7 - row]);
                }
            }
            B_VR => {
                output[12] = scalar_avg3(edge[1], edge[2], edge[3]);
                output[8] = scalar_avg3(edge[2], edge[3], edge[4]);
                output[13] = scalar_avg3(edge[3], edge[4], edge[5]);
                output[4] = scalar_avg3(edge[3], edge[4], edge[5]);
                output[9] = scalar_avg2(edge[4], edge[5]);
                output[0] = scalar_avg2(edge[4], edge[5]);
                output[14] = scalar_avg3(edge[4], edge[5], edge[6]);
                output[5] = scalar_avg3(edge[4], edge[5], edge[6]);
                output[10] = scalar_avg2(edge[5], edge[6]);
                output[1] = scalar_avg2(edge[5], edge[6]);
                output[15] = scalar_avg3(edge[5], edge[6], edge[7]);
                output[6] = scalar_avg3(edge[5], edge[6], edge[7]);
                output[11] = scalar_avg2(edge[6], edge[7]);
                output[2] = scalar_avg2(edge[6], edge[7]);
                output[7] = scalar_avg3(edge[6], edge[7], edge[8]);
                output[3] = scalar_avg2(edge[7], edge[8]);
            }
            B_VL => {
                output[0] = scalar_avg2(top[0], top[1]);
                output[4] = scalar_avg3(top[0], top[1], top[2]);
                output[8] = scalar_avg2(top[1], top[2]);
                output[1] = scalar_avg2(top[1], top[2]);
                output[5] = scalar_avg3(top[1], top[2], top[3]);
                output[12] = scalar_avg3(top[1], top[2], top[3]);
                output[9] = scalar_avg2(top[2], top[3]);
                output[2] = scalar_avg2(top[2], top[3]);
                output[13] = scalar_avg3(top[2], top[3], top[4]);
                output[6] = scalar_avg3(top[2], top[3], top[4]);
                output[10] = scalar_avg2(top[3], top[4]);
                output[3] = scalar_avg2(top[3], top[4]);
                output[14] = scalar_avg3(top[3], top[4], top[5]);
                output[7] = scalar_avg3(top[3], top[4], top[5]);
                output[11] = scalar_avg3(top[4], top[5], top[6]);
                output[15] = scalar_avg3(top[5], top[6], top[7]);
            }
            B_HD => {
                output[12] = scalar_avg2(edge[0], edge[1]);
                output[13] = scalar_avg3(edge[0], edge[1], edge[2]);
                output[8] = scalar_avg2(edge[1], edge[2]);
                output[14] = scalar_avg2(edge[1], edge[2]);
                output[9] = scalar_avg3(edge[1], edge[2], edge[3]);
                output[15] = scalar_avg3(edge[1], edge[2], edge[3]);
                output[10] = scalar_avg2(edge[2], edge[3]);
                output[4] = scalar_avg2(edge[2], edge[3]);
                output[11] = scalar_avg3(edge[2], edge[3], edge[4]);
                output[5] = scalar_avg3(edge[2], edge[3], edge[4]);
                output[6] = scalar_avg2(edge[3], edge[4]);
                output[0] = scalar_avg2(edge[3], edge[4]);
                output[7] = scalar_avg3(edge[3], edge[4], edge[5]);
                output[1] = scalar_avg3(edge[3], edge[4], edge[5]);
                output[2] = scalar_avg3(edge[4], edge[5], edge[6]);
                output[3] = scalar_avg3(edge[5], edge[6], edge[7]);
            }
            B_HU => {
                output[0] = scalar_avg2(left[0], left[1]);
                output[1] = scalar_avg3(left[0], left[1], left[2]);
                output[2] = scalar_avg2(left[1], left[2]);
                output[4] = scalar_avg2(left[1], left[2]);
                output[3] = scalar_avg3(left[1], left[2], left[3]);
                output[5] = scalar_avg3(left[1], left[2], left[3]);
                output[6] = scalar_avg2(left[2], left[3]);
                output[8] = scalar_avg2(left[2], left[3]);
                output[7] = scalar_avg3(left[2], left[3], left[3]);
                output[9] = scalar_avg3(left[2], left[3], left[3]);
                output[10..].fill(left[3]);
            }
            _ => unreachable!("only the ten subblock modes are encoded"),
        }
        output
    }

    #[test]
    fn all_subblock_modes_match_scalar_equations() {
        let mut sets = vec![
            ([127; 8], [129; 4], 127),
            ([100; 8], [50; 4], 60),
            ([0; 8], [0; 4], 0),
            ([255; 8], [255; 4], 255),
            (
                [0, 1, 127, 128, 254, 255, 0, 255],
                [255, 254, 128, 127],
                128,
            ),
            ([254, 255, 0, 1, 128, 127, 255, 0], [0, 1, 254, 255], 1),
        ];
        let mut state = 0x1234_5678u32;
        for _ in 0..512 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let mut top = [0; 8];
            for value in &mut top {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *value = state as u8;
            }
            let mut left = [0; 4];
            for value in &mut left {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *value = state as u8;
            }
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            sets.push((top, left, state as u8));
        }
        for (top, left, corner) in sets {
            let borders = Borders::new(&top, &left, corner);
            for mode in [B_DC, B_TM, B_VE, B_HE, B_LD, B_RD, B_VR, B_VL, B_HD, B_HU] {
                assert_eq!(
                    borders.block(mode),
                    scalar_subblock(mode, &top, &left, corner),
                    "mode {mode}, corners {top:?} {left:?} {corner}"
                );
            }
        }
    }
}
