//! Decisions and quantized coefficients shared by VP8 mode and token coding.

use crate::token::{BLOCK_COEFFICIENTS, ZIGZAG};

/// Luma samples per side of a VP8 macroblock.
pub(crate) const MACROBLOCK_SIDE: usize = 16;
/// Chroma samples per side of a VP8 macroblock.
pub(crate) const CHROMA_SIDE: usize = MACROBLOCK_SIDE / 2;
/// Luma, U, and V coefficient blocks per macroblock.
const BLOCKS: usize = 24;
/// Most scan positions one macroblock can spool.
const MAX_VALUES: usize = BLOCKS * BLOCK_COEFFICIENTS;
/// Values per spool chunk. Whole macroblocks stay inside one chunk.
const CHUNK: usize = 32_768;

#[derive(Clone, Copy)]
pub(crate) enum Luma {
    Intra16 { mode: u8, dc: [i16; 16] },
    Intra4 { modes: [u8; 16] },
}

/// One macroblock's coefficients before sparse packing.
#[derive(Clone, Copy)]
pub(crate) struct DenseMacroblock {
    pub(crate) luma: Luma,
    // Intra16 DC terms live in `luma`; its `y` blocks keep their DC slots zero.
    pub(crate) y: [[i16; 16]; 16],
    pub(crate) chroma_mode: u8,
    pub(crate) u: [[i16; 16]; 4],
    pub(crate) v: [[i16; 16]; 4],
}

/// A macroblock's coefficients as per-block scan prefixes in a shared spool.
pub(crate) struct Macroblock {
    pub(crate) luma: Luma,
    pub(crate) chroma_mode: u8,
    /// Scan-prefix length (highest nonzero scan position + 1) of each y, u, v block.
    pub(crate) lengths: [u8; BLOCKS],
    pub(crate) offset: u32,
}

impl Macroblock {
    pub(crate) fn pack(dense: DenseMacroblock, spool: &mut CoefficientSpool) -> Self {
        let offset = spool.begin_macroblock();
        let mut lengths = [0; BLOCKS];
        let mut index = 0;
        for block in &dense.y {
            lengths[index] = spool.push_block(block);
            index += 1;
        }
        for block in &dense.u {
            lengths[index] = spool.push_block(block);
            index += 1;
        }
        for block in &dense.v {
            lengths[index] = spool.push_block(block);
            index += 1;
        }
        Self {
            luma: dense.luma,
            chroma_mode: dense.chroma_mode,
            lengths,
            offset,
        }
    }

    /// Point this macroblock at the same coefficients in a concatenated spool.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn rebase(&self, base: u32) -> Self {
        Self {
            luma: self.luma,
            chroma_mode: self.chroma_mode,
            lengths: self.lengths,
            offset: self.offset + base,
        }
    }
}

/// Append-only, chunked coefficient storage. Chunks avoid reallocation copies, so peak memory
/// tracks the packed coefficients instead of a doubled capacity.
pub(crate) struct CoefficientSpool {
    chunks: Vec<Box<[i16]>>,
    used: usize,
}

impl CoefficientSpool {
    pub(crate) fn new() -> Self {
        Self {
            chunks: Vec::new(),
            used: 0,
        }
    }

    /// Start one macroblock and return its base spool index.
    pub(crate) fn begin_macroblock(&mut self) -> u32 {
        if self.chunks.is_empty() || self.used + MAX_VALUES > CHUNK {
            self.chunks.push(vec![0; CHUNK].into_boxed_slice());
            self.used = 0;
        }
        ((self.chunks.len() - 1) * CHUNK + self.used) as u32
    }

    /// Append one block's scan prefix and return its length.
    fn push_block(&mut self, levels: &[i16; 16]) -> u8 {
        let mut length = 0;
        for position in (0..16).rev() {
            if levels[ZIGZAG[position]] != 0 {
                length = position + 1;
                break;
            }
        }
        let chunk = self.chunks.len() - 1;
        let values = &mut self.chunks[chunk][self.used..self.used + length];
        for (position, value) in values.iter_mut().enumerate() {
            *value = levels[ZIGZAG[position]];
        }
        self.used += length;
        length as u8
    }

    pub(crate) fn block(&self, start: usize, length: u8) -> &[i16] {
        let chunk = start / CHUNK;
        let offset = start % CHUNK;
        &self.chunks[chunk][offset..offset + usize::from(length)]
    }

    /// Move another spool's chunks to the end of this one and return its base index.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn absorb(&mut self, other: CoefficientSpool) -> u32 {
        let base = (self.chunks.len() * CHUNK) as u32;
        self.chunks.extend(other.chunks);
        // Later pushes must start a fresh chunk; `other`'s last chunk may be partially used.
        self.used = CHUNK;
        base
    }
}
