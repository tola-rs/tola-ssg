//! Row storage for one evaluation stage: a flat buffer, its width, and the order proof later passes
//! rely on.
//!
//! A block is a batch, not a relation: it may hold duplicates and out-of-order rows until the
//! answer leaves, so only a normalized block may be searched or merged.
use super::ensure_not_cancelled;
use crate::Error;
use crate::index::gallop;

/// Rows of one clause stage as they accumulate; `strictly_sorted` is a conservative proof that
/// turns false whenever an append cannot show the new tail sorts above the old.
pub(super) struct RowBlock<C> {
    values: Vec<C>,
    width: usize,
    len: usize,
    strictly_sorted: bool,
}
impl<C> RowBlock<C> {
    pub(super) fn single_row(values: Vec<C>) -> Self {
        Self {
            width: values.len(),
            values,
            len: 1,
            strictly_sorted: true,
        }
    }
    pub(super) fn new(width: usize) -> Self {
        Self {
            values: Vec::new(),
            width,
            len: 0,
            strictly_sorted: true,
        }
    }
    pub(super) fn push(&mut self, values: impl IntoIterator<Item = C>) {
        self.values.extend(values);
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
    pub(super) fn row(&self, index: usize) -> &[C] {
        &self.values[index * self.width..(index + 1) * self.width]
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = &[C]> {
        (0..self.len).map(|index| self.row(index))
    }
    pub(super) fn append_block(&mut self, other: &Self)
    where
        C: Copy,
    {
        self.values.extend_from_slice(&other.values);
        self.len += other.len;
        self.strictly_sorted = self.len <= 1;
    }
    pub(super) fn has_capacity_for(&self, other: &Self) -> bool {
        self.values.capacity() - self.values.len() >= other.values.len()
    }
    fn append_values(&mut self, other: Self) {
        self.values.extend(other.values);
        self.len += other.len;
    }
}
impl<C: Ord + Copy> RowBlock<C> {
    pub(super) fn is_strictly_sorted(&self) -> bool {
        self.strictly_sorted
    }
    /// Appends the rows `append` writes, which are strictly sorted among themselves when
    /// `run_sorted`. The order proof survives only when the previous rows and the run are each
    /// strictly sorted and the seam between them is strictly ordered.
    pub(super) fn append_run<E>(
        &mut self,
        run_sorted: bool,
        append: impl FnOnce(&mut Self) -> Result<(), E>,
    ) -> Result<(), E> {
        let start = self.len;
        let previous = self.strictly_sorted;
        self.strictly_sorted = false;
        append(self)?;
        if self.len == start {
            self.strictly_sorted = previous;
            return Ok(());
        }
        let appended_sorted = run_sorted || self.len == start + 1;
        let seam_sorted = start == 0 || self.row(start - 1) < self.row(start);
        self.strictly_sorted = previous && appended_sorted && seam_sorted;
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

    /// Whether the rows are strictly increasing: the stored proof, or a scan over the rows when
    /// the proof has been invalidated.
    fn is_strictly_increasing(
        &self,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<bool, Error> {
        if !self.strictly_sorted {
            for index in 1..self.len {
                ensure_not_cancelled(is_cancelled)?;
                if self.row(index - 1) >= self.row(index) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Position of the first row at or after `start` that is not below `target`; the block must
    /// be strictly increasing.
    fn lower_bound(&self, start: usize, target: &[C]) -> usize {
        // Keys usually arrive after the whole block, which the exponential probe would scan.
        if start == self.len || self.row(start) >= target {
            return start;
        }
        if self.row(self.len - 1) < target {
            return self.len;
        }
        gallop(self.len, start, |index| self.row(index) < target)
    }

    /// Whether `row` occurs in this strictly increasing block.
    fn contains(&self, row: &[C]) -> bool {
        let index = self.lower_bound(0, row);
        index < self.len && self.row(index) == row
    }

    /// Removes the rows that occur in any `known` block and leaves this block strictly increasing.
    /// Every `known` block must itself be strictly increasing.
    pub(super) fn retain_unseen(
        &mut self,
        known: &[Self],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        // A single row is kept or dropped whole and needs no ordering work.
        if self.len == 1 {
            if known.iter().any(|batch| batch.contains(self.row(0))) {
                self.values.clear();
                self.len = 0;
            }
            self.strictly_sorted = true;
            return Ok(());
        }
        let sorted = self.is_strictly_increasing(is_cancelled)?;
        if sorted && known.is_empty() {
            self.strictly_sorted = true;
            return Ok(());
        }
        // `order` lists the rows in ascending row order, so one cursor walks each known block.
        let mut order = if sorted {
            (0..self.len).collect()
        } else {
            self.sorted_order(is_cancelled)?
        };
        for batch in known {
            let mut cursor = 0;
            let mut kept = 0;
            for position in 0..order.len() {
                ensure_not_cancelled(is_cancelled)?;
                let index = order[position];
                let target = self.row(index);
                cursor = batch.lower_bound(cursor, target);
                if cursor == batch.len || batch.row(cursor) != target {
                    order[kept] = index;
                    kept += 1;
                }
            }
            order.truncate(kept);
        }
        self.reorder_sorted(order, is_cancelled)
    }
    /// Sorts the rows and drops duplicates, leaving the block strictly increasing.
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
        let order = self.sorted_order(is_cancelled)?;
        self.reorder_sorted(order, is_cancelled)
    }
    /// The rows in ascending row order; equal rows keep the earliest arrival, so this is the
    /// order a stable sort would produce.
    fn sorted_order(&self, is_cancelled: &mut impl FnMut() -> bool) -> Result<Vec<usize>, Error> {
        ensure_not_cancelled(is_cancelled)?;
        let mut order: Vec<usize> = (0..self.len).collect();
        order.sort_unstable_by(|&left, &right| {
            self.row(left)
                .cmp(self.row(right))
                .then_with(|| left.cmp(&right))
        });
        order.dedup_by(|left, right| self.row(*left) == self.row(*right));
        ensure_not_cancelled(is_cancelled)?;
        Ok(order)
    }

    /// Rearranges the rows into `order`, which lists them in row order.
    fn reorder_sorted(
        &mut self,
        order: Vec<usize>,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        if order.len() == self.len && order.iter().copied().eq(0..self.len) {
            self.strictly_sorted = true;
            return Ok(());
        }
        let mut values = Vec::with_capacity(order.len() * self.width);
        for &index in &order {
            ensure_not_cancelled(is_cancelled)?;
            values.extend_from_slice(self.row(index));
        }
        self.values = values;
        self.len = order.len();
        self.strictly_sorted = true;
        Ok(())
    }
    // Both blocks are non-empty and already deduplicated against each other; zero-width rows are
    // equal to every other row, so they never reach this merge with both sides populated.
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
        let mut values = Vec::with_capacity(self.values.len() + other.values.len());
        let mut left = self.values.into_iter();
        let mut right = other.values.into_iter();
        let (mut left_len, mut right_len) = (self.len, other.len);
        while left_len != 0 && right_len != 0 {
            ensure_not_cancelled(is_cancelled)?;
            if left.as_slice()[..width] <= right.as_slice()[..width] {
                values.extend(left.by_ref().take(width));
                left_len -= 1;
            } else {
                values.extend(right.by_ref().take(width));
                right_len -= 1;
            }
        }
        values.extend(left);
        values.extend(right);
        Ok(Self {
            values,
            width,
            len,
            strictly_sorted: true,
        })
    }
}
