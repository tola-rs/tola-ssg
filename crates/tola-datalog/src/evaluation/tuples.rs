//! Flat row batches with conservative strict-ordering proofs.

use super::ensure_not_cancelled;
use crate::Error;
use crate::index::gallop;
use std::ops::Range;

pub(super) struct RowBlock<C> {
    vals: Vec<C>,
    width: usize,
    len: usize,
    strictly_sorted: bool,
}

impl<C> RowBlock<C> {
    pub(super) fn single_row(vals: Vec<C>) -> Self {
        Self {
            width: vals.len(),
            vals,
            len: 1,
            strictly_sorted: true,
        }
    }

    pub(super) fn new(width: usize) -> Self {
        Self {
            vals: Vec::new(),
            width,
            len: 0,
            strictly_sorted: true,
        }
    }

    pub(super) fn push(&mut self, vals: impl IntoIterator<Item = C>) {
        self.vals.extend(vals);
        self.len += 1;
        self.strictly_sorted = self.len <= 1;
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn width(&self) -> usize {
        self.width
    }

    pub(super) fn row(&self, idx: usize) -> &[C] {
        &self.vals[idx * self.width..(idx + 1) * self.width]
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &[C]> {
        (0..self.len).map(|idx| self.row(idx))
    }

    pub(super) fn append_block(&mut self, other: &Self)
    where
        C: Copy,
    {
        self.vals.extend_from_slice(&other.vals);
        self.len += other.len;
        self.strictly_sorted = self.len <= 1;
    }

    pub(super) fn has_capacity_for(&self, other: &Self) -> bool {
        self.vals.capacity() - self.vals.len() >= other.vals.len()
    }

    fn append_values(&mut self, other: Self) {
        self.vals.extend(other.vals);
        self.len += other.len;
    }
}

impl<C: Ord + Copy> RowBlock<C> {
    pub(super) fn is_strictly_sorted(&self) -> bool {
        self.strictly_sorted
    }

    pub(super) fn append_run<E>(
        &mut self,
        run_sorted: bool,
        append: impl FnOnce(&mut Self) -> Result<(), E>,
    ) -> Result<(), E> {
        let start = self.len;
        let prev = self.strictly_sorted;
        self.strictly_sorted = false;
        append(self)?;
        if self.len == start {
            self.strictly_sorted = prev;
            return Ok(());
        }
        self.strictly_sorted = prev
            && (run_sorted || self.len == start + 1)
            && (start == 0 || self.row(start - 1) < self.row(start));
        Ok(())
    }

    pub(super) fn extend(&mut self, other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        let sorted = if other.is_empty() {
            self.strictly_sorted
        } else {
            self.strictly_sorted && other.strictly_sorted && self.row(self.len - 1) < other.row(0)
        };
        self.append_values(other);
        self.strictly_sorted = sorted;
    }

    fn is_strictly_increasing(
        &self,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<bool, Error> {
        if !self.strictly_sorted {
            for idx in 1..self.len {
                ensure_not_cancelled(is_cancelled)?;
                if self.row(idx - 1) >= self.row(idx) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn lower_bound(&self, start: usize, target: &[C]) -> usize {
        if start == self.len || self.row(start) >= target {
            return start;
        }
        if self.row(self.len - 1) < target {
            return self.len;
        }
        gallop(self.len, start, |idx| self.row(idx) < target)
    }

    fn contains(&self, row: &[C]) -> bool {
        if self.width == 1 {
            let key = row[0];
            let first = self.vals[0].cmp(&key);
            if !first.is_lt() {
                return first.is_eq();
            }
            let last = self.vals[self.len - 1].cmp(&key);
            if !last.is_gt() {
                return last.is_eq();
            }
            return self.vals[1..self.len - 1].binary_search(&key).is_ok();
        }
        self.contains_from(&mut 0, row)
    }

    pub(super) fn retain_unseen(
        &mut self,
        known: &[Self],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.len == 1 {
            if known.iter().any(|b| b.contains(self.row(0))) {
                self.vals.clear();
                self.len = 0;
            }
            self.strictly_sorted = true;
            return Ok(());
        }
        if self.is_strictly_increasing(is_cancelled)? {
            return self.remove_known_rows(known, is_cancelled);
        }
        if self.width == 1 {
            self.sort_single_column(is_cancelled)?;
            return self.remove_known_rows(known, is_cancelled);
        }
        let mut order = self.sorted_order(is_cancelled)?;
        for batch in known {
            let mut cursor = 0;
            let mut kept = 0;
            for pos in 0..order.len() {
                ensure_not_cancelled(is_cancelled)?;
                let idx = order[pos];
                if !batch.contains_from(&mut cursor, self.row(idx)) {
                    order[kept] = idx;
                    kept += 1;
                }
            }
            order.truncate(kept);
        }
        self.reorder_sorted(order, is_cancelled)
    }

    fn remove_known_rows(
        &mut self,
        known: &[Self],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        let orig_len = self.len;
        for batch in known {
            if self.is_empty() {
                break;
            }
            if self.row(self.len - 1) < batch.row(0) || batch.row(batch.len - 1) < self.row(0) {
                continue;
            }
            let start = self.lower_bound(0, batch.row(0));
            let mut cursor = 0;
            let mut kept = start;
            for idx in start..self.len {
                ensure_not_cancelled(is_cancelled)?;
                if cursor == batch.len {
                    self.copy_rows(idx..self.len, kept);
                    kept += self.len - idx;
                    break;
                }
                if !batch.contains_from(&mut cursor, self.row(idx)) {
                    self.copy_rows(idx..idx + 1, kept);
                    kept += 1;
                }
            }
            self.len = kept;
            self.vals.truncate(kept * self.width);
        }
        if self.len != 0 && self.len < orig_len / 2 {
            self.vals.shrink_to_fit();
        }
        self.strictly_sorted = true;
        Ok(())
    }

    pub(super) fn contains_from(&self, cursor: &mut usize, row: &[C]) -> bool {
        *cursor = self.lower_bound(*cursor, row);
        *cursor < self.len && self.row(*cursor) == row
    }

    fn copy_rows(&mut self, rows: Range<usize>, dst: usize) {
        if rows.start != dst {
            self.vals.copy_within(
                rows.start * self.width..rows.end * self.width,
                dst * self.width,
            );
        }
    }

    pub(super) fn sort_distinct(
        &mut self,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.width == 0 {
            self.len = self.len.min(1);
            self.strictly_sorted = true;
            return Ok(());
        }
        if self.is_strictly_increasing(is_cancelled)? {
            self.strictly_sorted = true;
            return Ok(());
        }
        if self.width == 1 {
            return self.sort_single_column(is_cancelled);
        }
        let order = self.sorted_order(is_cancelled)?;
        self.reorder_sorted(order, is_cancelled)
    }

    fn sort_single_column(&mut self, is_cancelled: &mut impl FnMut() -> bool) -> Result<(), Error> {
        ensure_not_cancelled(is_cancelled)?;
        // Stable sort preserves first arrival of equal constants.
        self.vals.sort();
        self.vals.dedup();
        self.len = self.vals.len();
        self.strictly_sorted = true;
        ensure_not_cancelled(is_cancelled)
    }

    fn sorted_order(&self, is_cancelled: &mut impl FnMut() -> bool) -> Result<Vec<usize>, Error> {
        ensure_not_cancelled(is_cancelled)?;
        let mut order: Vec<usize> = (0..self.len).collect();
        order.sort_unstable_by(|&l, &r| self.row(l).cmp(self.row(r)).then_with(|| l.cmp(&r)));
        order.dedup_by(|l, r| self.row(*l) == self.row(*r));
        ensure_not_cancelled(is_cancelled)?;
        Ok(order)
    }

    fn reorder_sorted(
        &mut self,
        order: Vec<usize>,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        if order.len() == self.len && order.iter().copied().eq(0..self.len) {
            self.strictly_sorted = true;
            return Ok(());
        }
        let mut vals = Vec::with_capacity(order.len() * self.width);
        for &idx in &order {
            ensure_not_cancelled(is_cancelled)?;
            vals.extend_from_slice(self.row(idx));
        }
        self.vals = vals;
        self.len = order.len();
        self.strictly_sorted = true;
        Ok(())
    }

    pub(super) fn merge_disjoint(
        mut self,
        mut other: Self,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Self, Error> {
        ensure_not_cancelled(is_cancelled)?;
        debug_assert!(self.width > 0, "zero-width rows never merge");
        if self.row(self.len - 1) < other.row(0) {
            self.append_values(other);
            return Ok(self);
        }
        if other.row(other.len - 1) < self.row(0) {
            other.append_values(self);
            return Ok(other);
        }
        let width = self.width;
        let len = self.len + other.len;
        let mut vals = Vec::with_capacity(self.vals.len() + other.vals.len());
        let mut left = self.vals.into_iter();
        let mut right = other.vals.into_iter();
        let (mut l_len, mut r_len) = (self.len, other.len);
        while l_len != 0 && r_len != 0 {
            ensure_not_cancelled(is_cancelled)?;
            if left.as_slice()[..width] <= right.as_slice()[..width] {
                vals.extend(left.by_ref().take(width));
                l_len -= 1;
            } else {
                vals.extend(right.by_ref().take(width));
                r_len -= 1;
            }
        }
        vals.extend(left);
        vals.extend(right);
        Ok(Self {
            vals,
            width,
            len,
            strictly_sorted: true,
        })
    }
}
