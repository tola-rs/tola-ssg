//! Relation row buffers and canonicalized frozen storage.

use crate::index::{ColumnIndex, RowIds};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::OnceLock;

pub(crate) struct RowBuffer<C> {
    arity: usize,
    len: usize,
    vals: Vec<C>,
}

pub(crate) struct RelationRows<C> {
    arity: usize,
    len: usize,
    vals: Box<[C]>,
    secondary: Vec<OnceLock<Vec<usize>>>,
}

impl<C> RowBuffer<C> {
    pub(crate) fn new(arity: usize) -> Self {
        Self {
            arity,
            len: 0,
            vals: Vec::new(),
        }
    }

    pub(crate) fn push_row(&mut self, vals: impl IntoIterator<Item = C>) -> usize {
        let start = self.vals.len();
        self.vals.extend(vals);
        self.len += 1;
        self.vals.len() - start
    }

    fn row(&self, idx: usize) -> &[C] {
        let start = idx * self.arity;
        &self.vals[start..start + self.arity]
    }

    fn swap_rows(&mut self, left: usize, right: usize) {
        for col in 0..self.arity {
            self.vals
                .swap(left * self.arity + col, right * self.arity + col);
        }
    }
}

impl<C: Ord> RowBuffer<C> {
    /// Sort lexicographically, deduplicate keeping first arrival, and freeze into `RelationRows`.
    pub(crate) fn canonicalize(mut self) -> RelationRows<C> {
        if self.arity == 0 {
            self.len = usize::from(self.len != 0);
        } else if self.len > 1 {
            if !(1..self.len).all(|i| self.row(i - 1) <= self.row(i)) {
                let order = self.row_order();
                self.apply_row_order(order);
            }
            self.keep_first_of_each_run();
        }
        RelationRows {
            arity: self.arity,
            len: self.len,
            vals: self.vals.into_boxed_slice(),
            secondary: (1..self.arity).map(|_| OnceLock::new()).collect(),
        }
    }

    fn row_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.len).collect();
        order.sort_unstable_by(|&l, &r| self.row(l).cmp(self.row(r)).then_with(|| l.cmp(&r)));
        order
    }

    fn apply_row_order(&mut self, mut order: Vec<usize>) {
        for pos in 0..order.len() {
            let mut row = pos;
            while order[row] != pos {
                let src = order[row];
                self.swap_rows(row, src);
                order[row] = row;
                row = src;
            }
            order[row] = row;
        }
    }

    fn keep_first_of_each_run(&mut self) {
        let mut kept = 1;
        for row in 1..self.len {
            if self.row(kept - 1) != self.row(row) {
                if kept != row {
                    self.swap_rows(kept, row);
                }
                kept += 1;
            }
        }
        self.len = kept;
        self.vals.truncate(kept * self.arity);
    }
}

impl<C> RelationRows<C> {
    pub(crate) fn row_ids(&self) -> RowIds<'_> {
        RowIds::Range(0..self.len)
    }

    pub(crate) fn row(&self, idx: usize) -> &[C] {
        let start = idx * self.arity;
        &self.vals[start..start + self.arity]
    }
}

impl<C: Ord> RelationRows<C> {
    pub(crate) fn matching_prefix<'a, 'b>(
        &self,
        rows: RowIds<'a>,
        cols: Range<usize>,
        key: impl Fn(usize) -> &'b C,
    ) -> RowIds<'a>
    where
        C: 'b,
    {
        let expected: SmallVec<[&'b C; 4]> = cols.clone().map(&key).collect();
        let first_col = cols.start;
        let cmp_row = |row: usize| {
            let cells = self.row(row);
            (0..expected.len())
                .map(|c| cells[first_col + c].cmp(expected[c]))
                .find(|ord| !ord.is_eq())
                .unwrap_or(Ordering::Equal)
        };
        let first = rows.partition_point(|row| cmp_row(row).is_lt());
        let last = rows.gallop(first, |row| !cmp_row(row).is_gt());
        rows.slice(first..last)
    }

    pub(crate) fn ordered_column<'a>(&'a self, col: usize, rows: RowIds<'a>) -> ColumnIndex<'a, C> {
        ColumnIndex::new(&self.vals, self.arity, col, rows)
    }

    pub(crate) fn column(&self, col: usize) -> ColumnIndex<'_, C> {
        let rows = if col == 0 {
            self.row_ids()
        } else {
            RowIds::Indexed(self.column_order(col))
        };
        ColumnIndex::new(&self.vals, self.arity, col, rows)
    }

    fn column_order(&self, col: usize) -> &[usize] {
        let cache = &self.secondary[col - 1];
        if let Some(order) = cache.get() {
            return order;
        }
        // Sort outside `OnceLock::get_or_init` so reentrant `C::cmp` cannot deadlock.
        let mut order: Vec<usize> = (0..self.len).collect();
        order.sort_unstable_by(|&l, &r| {
            self.row(l)[col]
                .cmp(&self.row(r)[col])
                .then_with(|| l.cmp(&r))
        });
        cache.get_or_init(|| order)
    }
}

impl<C: PartialEq> PartialEq for RelationRows<C> {
    fn eq(&self, other: &Self) -> bool {
        self.arity == other.arity && self.len == other.len && self.vals == other.vals
    }
}

impl<C: Eq> Eq for RelationRows<C> {}

impl<C: Hash> Hash for RelationRows<C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.arity.hash(state);
        self.len.hash(state);
        self.vals.hash(state);
    }
}

impl<C: std::fmt::Debug> std::fmt::Debug for RelationRows<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelationRows")
            .field("arity", &self.arity)
            .field("len", &self.len)
            .field("values", &self.vals)
            .finish()
    }
}
