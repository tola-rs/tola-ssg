//! Decisions and quantized coefficients shared by VP8 mode and token coding.

use crate::token::{BLOCK_COEFFICIENTS, ZIGZAG, last_nonzero};

/// Luma samples per side of a VP8 macroblock.
pub(crate) const MACROBLOCK_SIDE: usize = 16;
/// Chroma samples per side of a VP8 macroblock.
pub(crate) const CHROMA_SIDE: usize = MACROBLOCK_SIDE / 2;
/// Y2, luma, U, and V coefficient blocks per macroblock.
const BLOCKS: usize = 25;
/// Most scan positions one macroblock can spool.
const MAX_VALUES: usize = BLOCKS * BLOCK_COEFFICIENTS;
/// Values per spool chunk. Whole macroblocks stay inside one chunk.
const CHUNK: usize = 32_768;
// A 16,383² frame has at most 1,024² macroblocks. Even with chunk gaps,
// 400 coefficients per macroblock fit below this offset bit.
const WIDE_OFFSET: u32 = 1 << 31;

#[derive(Clone, Copy)]
pub(crate) enum Luma {
    Intra16 { mode: u8 },
    Intra4 { modes: [u8; 16] },
}

/// One macroblock's coefficients before sparse packing.
#[derive(Clone, Copy)]
pub(crate) struct DenseMacroblock {
    pub(crate) luma: Luma,
    // Intra16 DC terms live in Y2; its y blocks keep their DC slots zero.
    pub(crate) y2: [i16; 16],
    pub(crate) y: [[i16; 16]; 16],
    pub(crate) chroma_mode: u8,
    pub(crate) u: [[i16; 16]; 4],
    pub(crate) v: [[i16; 16]; 4],
}

/// A macroblock's coefficients as per-block scan prefixes in a shared spool.
#[derive(Clone, Copy)]
pub(crate) struct Macroblock {
    pub(crate) luma: Luma,
    pub(crate) chroma_mode: u8,
    /// Scan-prefix lengths in VP8 block order. Intra4 has an empty Y2 prefix.
    pub(crate) lengths: [u8; BLOCKS],
    // Zero denotes an empty span. Other offsets are one-based, with the width in the high bit.
    offset: u32,
}

impl Macroblock {
    pub(crate) fn has_residual(&self) -> bool {
        self.offset != 0
    }

    pub(crate) fn pack(dense: DenseMacroblock, spool: &mut CoefficientSpool) -> Self {
        let mut lengths = [0; BLOCKS];
        let blocks = std::iter::once(&dense.y2)
            .chain(&dense.y)
            .chain(&dense.u)
            .chain(&dense.v);
        let offset = spool.append(blocks, &mut lengths);
        Self {
            luma: dense.luma,
            chroma_mode: dense.chroma_mode,
            lengths,
            offset,
        }
    }
}

/// Append-only coefficient chunks. A macroblock uses signed bytes when every coefficient fits.
pub(crate) struct CoefficientSpool {
    narrow: Vec<Vec<i8>>,
    wide: Vec<Vec<i16>>,
    // Each pack overwrites its entire used prefix. Keep the buffer per strip.
    narrow_prefix: [i8; MAX_VALUES],
}

pub(crate) enum PackedLevels<'a> {
    Narrow(&'a [i8]),
    Wide(&'a [i16]),
}

impl CoefficientSpool {
    pub(crate) fn estimated_bytes(macroblocks: u64, strips: u64) -> u64 {
        let blocks_per_chunk = (CHUNK / MAX_VALUES) as u64;
        // Allow a partial chunk in each of the two spools per strip.
        let chunks = macroblocks.div_ceil(blocks_per_chunk) + 2 * strips - 1;
        chunks * CHUNK as u64 * size_of::<i16>() as u64
    }

    pub(crate) fn new() -> Self {
        Self {
            narrow: Vec::new(),
            wide: Vec::new(),
            narrow_prefix: [0; MAX_VALUES],
        }
    }

    fn append<'a>(
        &mut self,
        blocks: impl Iterator<Item = &'a [i16; 16]> + Clone,
        lengths: &mut [u8; BLOCKS],
    ) -> u32 {
        let mut count = 0;
        let mut shifted_levels = 0u16;
        for (block, length) in blocks.clone().zip(lengths.iter_mut()) {
            let prefix = &ZIGZAG[..prefix_len(block)];
            *length = prefix.len() as u8;
            for (&position, destination) in prefix.iter().zip(&mut self.narrow_prefix[count..]) {
                let level = block[position];
                // Adding 128 in unsigned arithmetic maps the i8 range to 0..=255.
                shifted_levels |= (level as u16).wrapping_add(128);
                *destination = level as i8;
            }
            count += prefix.len();
        }
        if count == 0 {
            return 0;
        }
        if shifted_levels <= u16::from(u8::MAX) {
            let offset = reserve_coefficients(&mut self.narrow, count);
            self.narrow
                .last_mut()
                .unwrap()
                .extend_from_slice(&self.narrow_prefix[..count]);
            return offset;
        }

        let offset = reserve_coefficients(&mut self.wide, count);
        let chunk = self.wide.last_mut().unwrap();
        for (block, &length) in blocks.zip(lengths.iter()) {
            chunk.extend(
                ZIGZAG[..usize::from(length)]
                    .iter()
                    .map(|&position| block[position]),
            );
        }
        offset | WIDE_OFFSET
    }

    /// The coefficient span starts at the macroblock offset and is partitioned by its lengths.
    pub(crate) fn levels(&self, macroblock: &Macroblock) -> PackedLevels<'_> {
        let offset = macroblock.offset;
        if offset == 0 {
            PackedLevels::Narrow(&[])
        } else if offset & WIDE_OFFSET == 0 {
            PackedLevels::Narrow(coefficient_tail(&self.narrow, offset))
        } else {
            PackedLevels::Wide(coefficient_tail(&self.wide, offset & !WIDE_OFFSET))
        }
    }
}

fn reserve_coefficients<T>(chunks: &mut Vec<Vec<T>>, count: usize) -> u32 {
    if chunks
        .last()
        .is_none_or(|chunk| chunk.len() + count > CHUNK)
    {
        chunks.push(Vec::new());
    }
    let index = chunks.len() - 1;
    let chunk = &mut chunks[index];
    let end = chunk.len() + count;
    if end > chunk.capacity() {
        chunk.reserve_exact(end.next_power_of_two() - chunk.len());
    }
    (index * CHUNK + chunk.len()) as u32 + 1
}

fn prefix_len(block: &[i16; 16]) -> usize {
    last_nonzero(block, 0).map_or(0, |position| position + 1)
}

fn coefficient_tail<T>(chunks: &[Vec<T>], offset: u32) -> &[T] {
    let offset = (offset - 1) as usize;
    &chunks[offset / CHUNK][offset % CHUNK..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficient_spans_preserve_signs_across_chunks() {
        let dense = |level| DenseMacroblock {
            luma: Luma::Intra16 { mode: 0 },
            y2: [level; 16],
            chroma_mode: 0,
            y: [[level; 16]; 16],
            u: [[level; 16]; 4],
            v: [[level; 16]; 4],
        };
        let mut spool = CoefficientSpool::new();
        let empty = Macroblock::pack(dense(0), &mut spool);
        assert!(matches!(spool.levels(&empty), PackedLevels::Narrow([])));
        assert!(!empty.has_residual());
        assert!(spool.narrow.is_empty() && spool.wide.is_empty());

        let mut packed = Vec::new();
        for _ in 0..=CHUNK / (2 * MAX_VALUES) {
            for level in [-128, 127, -129, 128] {
                packed.push((level, Macroblock::pack(dense(level), &mut spool)));
            }
        }
        for (level, macroblock) in packed {
            match spool.levels(&macroblock) {
                PackedLevels::Narrow(levels) => {
                    assert_eq!(
                        levels[..MAX_VALUES],
                        [i8::try_from(level).unwrap(); MAX_VALUES]
                    );
                }
                PackedLevels::Wide(levels) => {
                    assert!(i8::try_from(level).is_err());
                    assert_eq!(levels[..MAX_VALUES], [level; MAX_VALUES]);
                }
            }
        }
    }
}
