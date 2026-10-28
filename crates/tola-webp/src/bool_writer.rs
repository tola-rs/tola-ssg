//! The VP8 boolean arithmetic encoder (RFC 6386 §7.3).
//!
//! One partition is written at a time: every call narrows the current interval, and shifting the
//! range back to `[128, 255]` emits whole bytes. The trailing bytes carry the final interval in
//! [`BoolWriter::finish`].

/// Writes one bool-coded VP8 partition.
pub(crate) struct BoolWriter {
    output: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
}

impl BoolWriter {
    pub(crate) fn new() -> Self {
        Self {
            output: Vec::new(),
            range: 255,
            bottom: 0,
            bit_count: 24,
        }
    }

    /// Write `value`, whose probability of being false is `probability / 256`.
    #[inline(always)]
    pub(crate) fn write_bool(&mut self, probability: u8, value: bool) {
        debug_assert_ne!(probability, 0);
        let split = 1 + (((self.range - 1) * u32::from(probability)) >> 8);
        if value {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        if self.range >= 128 {
            return;
        }
        // One shift covers the common 64..=127 range.
        self.shift_one();
        while self.range < 128 {
            self.shift_one();
        }
    }

    #[inline]
    fn shift_one(&mut self) {
        self.range <<= 1;
        if self.bottom & (1 << 31) != 0 {
            self.propagate_carry();
        }
        self.bottom <<= 1;
        self.bit_count -= 1;
        if self.bit_count == 0 {
            self.output.push((self.bottom >> 24) as u8);
            self.bottom &= (1 << 24) - 1;
            self.bit_count = 8;
        }
    }

    /// Write `value` as `bits` literal bits, most significant first.
    pub(crate) fn write_literal(&mut self, value: u32, bits: u8) {
        for shift in (0..bits).rev() {
            self.write_bool(128, value & (1 << shift) != 0);
        }
    }

    /// Write one leaf of a VP8 tree, starting at `start` as the token coder does after a zero.
    pub(crate) fn write_tree_from(
        &mut self,
        tree: &[i8],
        probabilities: &[u8],
        value: u8,
        start: usize,
    ) {
        let mut path = [false; 8];
        let len = path_to(tree, start, value, &mut path, 0).expect("the value must be a tree leaf");
        let mut node = start;
        for branch in path.into_iter().take(len) {
            self.write_bool(probabilities[node >> 1], branch);
            node = tree[node + usize::from(branch)] as usize;
        }
    }

    /// Flush the final interval and return the partition bytes.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        let mut remaining = self.bit_count;
        let mut value = self.bottom;
        if remaining > 0 && value & (1_u32 << (32 - remaining)) != 0 {
            self.propagate_carry();
        }
        value <<= (remaining & 7) as u32;
        remaining >>= 3;
        while remaining > 0 {
            value <<= 8;
            remaining -= 1;
        }
        for _ in 0..4 {
            self.output.push((value >> 24) as u8);
            value <<= 8;
        }
        self.output
    }

    /// Propagate a carry into the bytes already written.
    #[cold]
    fn propagate_carry(&mut self) {
        for byte in self.output.iter_mut().rev() {
            if *byte == u8::MAX {
                *byte = 0;
            } else {
                *byte += 1;
                return;
            }
        }
        panic!("bool-writer carry escaped its partition");
    }
}

/// Predicts one partition's byte length without materialising `bottom` or carry bytes.
///
/// Byte count depends only on how often renormalization drains `bit_count`: each drain appends
/// one byte, and [`BoolWriter::finish`] always appends four. Carries rewrite earlier bytes but
/// never add or remove one.
#[derive(Clone)]
pub(crate) struct LengthWriter {
    range: u32,
    bit_count: i32,
    written: usize,
}

impl LengthWriter {
    pub(crate) fn new() -> Self {
        Self {
            range: 255,
            bit_count: 24,
            written: 0,
        }
    }

    /// Write `value`, whose probability of being false is `probability / 256`.
    #[inline(always)]
    pub(crate) fn write_bool(&mut self, probability: u8, value: bool) {
        debug_assert_ne!(probability, 0);
        let split = 1 + (((self.range - 1) * u32::from(probability)) >> 8);
        if value {
            self.range -= split;
        } else {
            self.range = split;
        }
        while self.range < 128 {
            self.range <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.written += 1;
                self.bit_count = 8;
            }
        }
    }

    /// The partition length in bytes, including [`BoolWriter::finish`]'s four-byte flush.
    pub(crate) fn finish(self) -> usize {
        self.written + 4
    }
}

/// Find the branches from `node` to the leaf carrying `value`.
pub(crate) fn tree_path(
    tree: &[i8],
    node: usize,
    value: u8,
    path: &mut [bool; 8],
) -> Option<usize> {
    path_to(tree, node, value, path, 0)
}

/// Packed branches and lengths for each leaf; zero length means the leaf is unreachable.
pub(crate) const fn tree_paths<const N: usize>(tree: &[i8], start: usize) -> [(u8, u8); N] {
    const fn fill<const N: usize>(
        tree: &[i8],
        node: usize,
        bits: u8,
        len: u8,
        paths: &mut [(u8, u8); N],
    ) {
        assert!(len < 8, "VP8 tree paths fit in eight bits");
        let mut branch = 0;
        while branch < 2 {
            let child = tree[node + branch];
            let bits = (bits << 1) | branch as u8;
            if child <= 0 {
                paths[child.unsigned_abs() as usize] = (bits, len + 1);
            } else {
                fill(tree, child as usize, bits, len + 1, paths);
            }
            branch += 1;
        }
    }
    let mut paths = [(0, 0); N];
    fill(tree, start, 0, 0, &mut paths);
    paths
}

fn path_to(
    tree: &[i8],
    node: usize,
    value: u8,
    path: &mut [bool; 8],
    depth: usize,
) -> Option<usize> {
    for branch in [false, true] {
        let child = tree[node + usize::from(branch)];
        path[depth] = branch;
        if child <= 0 {
            if child.unsigned_abs() == value {
                return Some(depth + 1);
            }
        } else if let Some(len) = path_to(tree, child as usize, value, path, depth + 1) {
            return Some(len);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = *state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    /// The length model exists so a partition can be priced without writing it. It is only
    /// correct when the number it predicts is the number of bytes the writer emits, measured
    /// both mid-stream (where the pending byte is still unpadded) and after finishing.
    #[test]
    fn length_model_matches_written_bytes() {
        assert_eq!(
            LengthWriter::new().finish(),
            BoolWriter::new().finish().len()
        );

        // Every probability, so no branch's arithmetic is left unpriced.
        for probability in 1..=u8::MAX {
            for seed in 0..4 {
                let mut state = (u64::from(probability) << 32) | seed;
                let mut writer = BoolWriter::new();
                let mut length = LengthWriter::new();
                for step in 0..=512 {
                    let value = splitmix(&mut state) & 1 != 0;
                    assert_length_tracks(
                        &mut writer,
                        &mut length,
                        probability,
                        value,
                        step,
                        "random bits",
                    );
                }
                assert_eq!(
                    length.finish(),
                    writer.finish().len(),
                    "probability {probability}, seed {seed}"
                );
            }
        }

        // A finished stream, across every byte boundary a partition is priced at.
        for count in [1, 7, 8, 9, 23, 24, 25, 100, 1_000] {
            let mut state = count as u64;
            let mut writer = BoolWriter::new();
            let mut length = LengthWriter::new();
            for step in 0..count {
                let value = splitmix(&mut state) & 1 != 0;
                assert_length_tracks(
                    &mut writer,
                    &mut length,
                    128,
                    value,
                    step,
                    "finished stream",
                );
            }
            assert_eq!(length.finish(), writer.finish().len(), "count {count}");
        }

        // A literal, written as eight equiprobable bits.
        let mut writer = BoolWriter::new();
        let mut length = LengthWriter::new();
        for value in 0..=255 {
            writer.write_literal(value, 8);
            for shift in (0..8).rev() {
                let bit = value & (1 << shift) != 0;
                length.write_bool(128, bit);
                assert_eq!(length.clone().finish(), writer.output.len() + 4);
            }
        }
        assert_eq!(length.finish(), writer.finish().len());
    }

    /// A bit written when the pending byte is not yet flushed costs the four bytes the encoder
    /// still holds, so the running length is the emitted bytes plus the pending word.
    fn assert_length_tracks(
        writer: &mut BoolWriter,
        length: &mut LengthWriter,
        probability: u8,
        value: bool,
        step: usize,
        label: &str,
    ) {
        writer.write_bool(probability, value);
        length.write_bool(probability, value);
        if step.is_multiple_of(16) {
            assert_eq!(
                length.clone().finish(),
                writer.output.len() + 4,
                "{label} probability {probability}, step {step}"
            );
        }
    }
}
