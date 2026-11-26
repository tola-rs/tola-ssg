//! VP8 boolean arithmetic encoder (RFC 6386 §7.3).

/// Writes one bool-coded VP8 partition.
pub(crate) struct BoolWriter {
    buf: Vec<u8>,
    range: u32,
    bottom: u32,
    bits: i32,
}

impl BoolWriter {
    pub(crate) fn new() -> Self {
        Self {
            buf: Vec::new(),
            range: 255,
            bottom: 0,
            bits: 24,
        }
    }

    #[inline(always)]
    pub(crate) fn write_bool(&mut self, prob: u8, value: bool) {
        debug_assert_ne!(prob, 0);
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        if value {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        if self.range < 128 {
            self.normalize();
        }
    }

    #[inline]
    fn normalize(&mut self) {
        let shift = self.range.leading_zeros() - (u32::BITS - u8::BITS);
        self.range <<= shift;
        let pre = shift.min(self.bits as u32);
        let carries = self.bottom >> (u32::BITS - pre);
        if carries != 0 {
            self.propagate_carry(carries.count_ones() as u8);
        }
        self.bottom <<= pre;
        self.bits -= shift as i32;
        if self.bits <= 0 {
            self.buf.push((self.bottom >> 24) as u8);
            // At most six shifts remain, so the retained 24 bits cannot carry again.
            self.bottom = (self.bottom & 0x00ff_ffff) << (shift - pre);
            self.bits += 8;
        }
    }

    pub(crate) fn write_literal(&mut self, value: u32, nbits: u8) {
        for shift in (0..nbits).rev() {
            self.write_bool(128, value & (1 << shift) != 0);
        }
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        let mut rem = self.bits;
        let mut val = self.bottom;
        if rem > 0 && val & (1_u32 << (32 - rem)) != 0 {
            self.propagate_carry(1);
        }
        val <<= (rem & 7) as u32;
        rem >>= 3;
        while rem > 0 {
            val <<= 8;
            rem -= 1;
        }
        for _ in 0..4 {
            self.buf.push((val >> 24) as u8);
            val <<= 8;
        }
        self.buf
    }

    #[cold]
    fn propagate_carry(&mut self, mut carry: u8) {
        for byte in self.buf.iter_mut().rev() {
            let (next, overflow) = byte.overflowing_add(carry);
            *byte = next;
            if !overflow {
                return;
            }
            carry = 1;
        }
        panic!("bool-writer carry escaped its partition");
    }
}

/// Tracks partition byte length without buffering carries (each drain emits 1 byte, finish emits 4).
#[derive(Clone)]
pub(crate) struct LengthWriter {
    range: u32,
    bits: i32,
    bytes: usize,
}

impl LengthWriter {
    pub(crate) fn new() -> Self {
        Self {
            range: 255,
            bits: 24,
            bytes: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn write_bool(&mut self, prob: u8, value: bool) {
        debug_assert_ne!(prob, 0);
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        if value {
            self.range -= split;
        } else {
            self.range = split;
        }
        if self.range < 128 {
            let shift = self.range.leading_zeros() - (u32::BITS - u8::BITS);
            self.range <<= shift;
            self.bits -= shift as i32;
            if self.bits <= 0 {
                self.bytes += 1;
                self.bits += 8;
            }
        }
    }

    pub(crate) fn finish(self) -> usize {
        self.bytes + 4
    }
}

#[cfg(test)]
pub(crate) fn tree_path(
    tree: &[i8],
    node: usize,
    value: u8,
    path: &mut [bool; 8],
) -> Option<usize> {
    path_to(tree, node, value, path, 0)
}

/// Packed `(bits, len)` branch paths per leaf; `len == 0` marks unreachable leaves.
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

#[cfg(test)]
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
        let mut v = *state;
        v = (v ^ (v >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        v = (v ^ (v >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        v ^ (v >> 31)
    }

    #[test]
    fn length_tracks_writer() {
        assert_eq!(
            LengthWriter::new().finish(),
            BoolWriter::new().finish().len()
        );

        for prob in 1..=u8::MAX {
            for seed in 0..4 {
                let mut state = (u64::from(prob) << 32) | seed;
                let mut w = BoolWriter::new();
                let mut ref_w = BoolWriter::new();
                let mut len = LengthWriter::new();
                for step in 0..=512 {
                    let bit = splitmix(&mut state) & 1 != 0;
                    write_bit_by_bit(&mut ref_w, prob, bit);
                    assert_length_tracks(&mut w, &mut len, prob, bit, step, "random");
                }
                let bytes = w.finish();
                assert_eq!(len.finish(), bytes.len(), "prob {prob}, seed {seed}");
                assert_eq!(ref_w.finish(), bytes, "prob {prob}, seed {seed}");
            }
        }

        for count in [1, 7, 8, 9, 23, 24, 25, 100, 1_000] {
            let mut state = count as u64;
            let mut w = BoolWriter::new();
            let mut ref_w = BoolWriter::new();
            let mut len = LengthWriter::new();
            for step in 0..count {
                let bits = splitmix(&mut state);
                let prob = (bits as u8).max(1);
                let bit = bits & 256 != 0;
                write_bit_by_bit(&mut ref_w, prob, bit);
                assert_length_tracks(&mut w, &mut len, prob, bit, step, "stream");
            }
            let bytes = w.finish();
            assert_eq!(len.finish(), bytes.len(), "count {count}");
            assert_eq!(ref_w.finish(), bytes, "count {count}");
        }

        let mut w = BoolWriter::new();
        let mut len = LengthWriter::new();
        for val in 0..=255 {
            w.write_literal(val, 8);
            for shift in (0..8).rev() {
                len.write_bool(128, val & (1 << shift) != 0);
                assert_eq!(len.clone().finish(), w.buf.len() + 4);
            }
        }
        assert_eq!(len.finish(), w.finish().len());
    }

    fn write_bit_by_bit(w: &mut BoolWriter, prob: u8, value: bool) {
        let split = 1 + (((w.range - 1) * u32::from(prob)) >> 8);
        if value {
            w.bottom = w.bottom.wrapping_add(split);
            w.range -= split;
        } else {
            w.range = split;
        }
        while w.range < 128 {
            w.range <<= 1;
            if w.bottom & (1 << 31) != 0 {
                w.propagate_carry(1);
            }
            w.bottom <<= 1;
            w.bits -= 1;
            if w.bits == 0 {
                w.buf.push((w.bottom >> 24) as u8);
                w.bottom &= 0x00ff_ffff;
                w.bits = 8;
            }
        }
    }

    fn assert_length_tracks(
        w: &mut BoolWriter,
        len: &mut LengthWriter,
        prob: u8,
        value: bool,
        step: usize,
        label: &str,
    ) {
        w.write_bool(prob, value);
        len.write_bool(prob, value);
        if step.is_multiple_of(16) {
            assert_eq!(
                len.clone().finish(),
                w.buf.len() + 4,
                "{label} prob {prob}, step {step}"
            );
        }
    }
}
