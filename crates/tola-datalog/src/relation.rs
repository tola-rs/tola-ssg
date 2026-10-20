//! Rows of one relation: `RowBuffer` while a caller is still supplying them, `RelationRows` once
//! they are frozen.
//!
//! Freezing is the only step that decides row order and duplicates, so everything downstream —
//! prefix matching, column indexes, equality — relies on it having happened, and a buffer is
//! never searched.
use crate::index::{ColumnIndex, RowIds};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::OnceLock;

/// Rows of one relation while they are collected: arrival order, duplicates kept.
pub(crate) struct RowBuffer<C> {
    arity: usize,
    len: usize,
    values: Vec<C>,
}

/// A relation's rows once frozen: sorted lexicographically and pairwise distinct, `arity` values
/// per row.
pub(crate) struct RelationRows<C> {
    arity: usize,
    /// Zero-arity rows hold no values, so the row count does not follow from `values`.
    len: usize,
    values: Box<[C]>,
    /// Row order by column for every column after the first, built on first use. Derived, so it is
    /// excluded from equality, hashing and `Debug`.
    secondary: Vec<OnceLock<Vec<usize>>>,
}

impl<C> RowBuffer<C> {
    pub(crate) fn new(arity: usize) -> Self {
        Self {
            arity,
            len: 0,
            values: Vec::new(),
        }
    }

    /// Reserves `values` cells; the caller sums the widths of the rows it will push.
    pub(crate) fn reserve(&mut self, values: usize) {
        self.values.reserve(values);
    }

    /// Appends one row and returns its actual width, so the caller can check it against the
    /// declared arity.
    pub(crate) fn push_row(&mut self, values: impl IntoIterator<Item = C>) -> usize {
        let start = self.values.len();
        self.values.extend(values);
        self.len += 1;
        self.values.len() - start
    }

    fn row(&self, index: usize) -> &[C] {
        let start = index * self.arity;
        &self.values[start..start + self.arity]
    }

    fn swap_rows(&mut self, left: usize, right: usize) {
        for column in 0..self.arity {
            self.values
                .swap(left * self.arity + column, right * self.arity + column);
        }
    }
}

impl<C: Ord> RowBuffer<C> {
    /// Sorts the rows, keeps the first of each run of equal rows and freezes the result.
    ///
    /// The order is lexicographic over the whole row; `matching_prefix` and the per-column index
    /// rely on it.
    pub(crate) fn canonicalize(mut self) -> RelationRows<C> {
        let row_order = self.row_order();
        self.apply_row_order(row_order);
        self.keep_first_of_each_run();
        RelationRows {
            arity: self.arity,
            len: self.len,
            values: self.values.into_boxed_slice(),
            secondary: (1..self.arity).map(|_| OnceLock::new()).collect(),
        }
    }

    /// For every position, the index of the row that belongs there. Ties are broken by source
    /// index, so a run of equal rows keeps the row that arrived first.
    fn row_order(&self) -> Vec<usize> {
        let mut row_order: Vec<usize> = (0..self.len).collect();
        row_order.sort_unstable_by(|&left, &right| {
            self.row(left)
                .cmp(self.row(right))
                .then_with(|| left.cmp(&right))
        });
        row_order
    }

    /// `row_order[row]` is where the row that belongs at `row` currently sits; swapping follows a
    /// cycle until it closes back on its start, whose position then holds the final row.
    fn apply_row_order(&mut self, mut row_order: Vec<usize>) {
        for position in 0..row_order.len() {
            let mut row = position;
            while row_order[row] != position {
                let source = row_order[row];
                self.swap_rows(row, source);
                row_order[row] = row;
                row = source;
            }
            row_order[row] = row;
        }
    }

    fn keep_first_of_each_run(&mut self) {
        if self.len == 0 {
            return;
        }
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
        self.values.truncate(kept * self.arity);
    }
}

impl<C> RelationRows<C> {
    pub(crate) fn row_ids(&self) -> RowIds<'_> {
        RowIds::Range(0..self.len)
    }

    pub(crate) fn row(&self, index: usize) -> &[C] {
        let start = index * self.arity;
        &self.values[start..start + self.arity]
    }
}

impl<C: Ord> RelationRows<C> {
    /// Rows of `rows` whose values in the leading `columns` equal the ones `key` returns.
    ///
    /// Those columns are fixed across the match, so the matches are one contiguous run and two
    /// binary searches bound it.
    pub(crate) fn matching_prefix<'a, 'b>(
        &self,
        rows: RowIds<'a>,
        columns: Range<usize>,
        key: impl Fn(usize) -> &'b C,
    ) -> RowIds<'a>
    where
        C: 'b,
    {
        // Read the leading keys once: every probe compares a row against them.
        let expected: SmallVec<[&'b C; 4]> = columns.clone().map(&key).collect();
        let first_column = columns.start;
        let compare = |row: usize| {
            let cells = self.row(row);
            (0..expected.len())
                .map(|column| cells[first_column + column].cmp(expected[column]))
                .find(|ordering| !ordering.is_eq())
                .unwrap_or(Ordering::Equal)
        };
        let first = rows.partition_point(|row| compare(row).is_lt());
        let rest = rows.slice(first..rows.len());
        let last = first + rest.partition_point(|row| !compare(row).is_gt());
        rows.slice(first..last)
    }

    /// `rows` ordered by `column`; the caller guarantees every earlier column is fixed, which is
    /// what makes those rows sorted by this column.
    pub(crate) fn ordered_column<'a>(
        &'a self,
        column: usize,
        rows: RowIds<'a>,
    ) -> ColumnIndex<'a, C> {
        ColumnIndex::new(&self.values, self.arity, column, rows)
    }

    /// The whole relation ordered by `column`; column 0 is the storage order itself, later columns
    /// come from the cache.
    pub(crate) fn column(&self, column: usize) -> ColumnIndex<'_, C> {
        let rows = if column == 0 {
            self.row_ids()
        } else {
            RowIds::Indexed(self.column_order(column))
        };
        ColumnIndex::new(&self.values, self.arity, column, rows)
    }

    /// The cached row order by `column`.
    fn column_order(&self, column: usize) -> &[usize] {
        let cache = &self.secondary[column - 1];
        if let Some(order) = cache.get() {
            return order;
        }
        // Sort before filling the slot: `C::cmp` is arbitrary code that may ask this relation for
        // this column's index again, and `get_or_init` would deadlock on the slot it is
        // initializing.
        let mut order: Vec<usize> = (0..self.len).collect();
        order.sort_unstable_by(|&left, &right| {
            self.row(left)[column]
                .cmp(&self.row(right)[column])
                .then_with(|| left.cmp(&right))
        });
        cache.get_or_init(|| order)
    }
}

impl<C: PartialEq> PartialEq for RelationRows<C> {
    fn eq(&self, other: &Self) -> bool {
        self.arity == other.arity && self.len == other.len && self.values == other.values
    }
}
impl<C: Eq> Eq for RelationRows<C> {}
impl<C: Hash> Hash for RelationRows<C> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        // `values` hashes its own length, so omitting `len` only lets zero-arity relations collide.
        self.arity.hash(hasher);
        self.values.hash(hasher);
    }
}
impl<C: std::fmt::Debug> std::fmt::Debug for RelationRows<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelationRows")
            .field("arity", &self.arity)
            .field("len", &self.len)
            .field("values", &self.values)
            .finish()
    }
}
