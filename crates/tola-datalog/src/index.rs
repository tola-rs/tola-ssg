//! Row selections over one relation's rows, and the primitive searches that walk one.
//!
//! Positions count within the selection, never as row ids: a caller crossing between the two goes
//! through `iter` or `slice`.
use std::ops::Range;

/// A selection of rows of one relation, in the order they must be visited: ascending row indices
/// for `Range`, a row permutation for `Indexed`.
pub(crate) enum RowIds<'a> {
    Range(Range<usize>),
    Indexed(&'a [usize]),
}
impl<'a> RowIds<'a> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Range(rows) => rows.len(),
            Self::Indexed(rows) => rows.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn at(&self, position: usize) -> usize {
        match self {
            Self::Range(rows) => rows.start + position,
            Self::Indexed(rows) => rows[position],
        }
    }

    /// `positions` is relative to this selection and must stay inside it.
    pub(crate) fn slice(&self, positions: Range<usize>) -> Self {
        debug_assert!(positions.end <= self.len(), "slice outside the selection");
        match self {
            Self::Range(rows) => {
                Self::Range(rows.start + positions.start..rows.start + positions.end)
            }
            Self::Indexed(rows) => {
                let rows = *rows;
                Self::Indexed(&rows[positions])
            }
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len()).map(|position| self.at(position))
    }

    /// Number of leading rows for which `before` holds; `before` must be true for a prefix of the
    /// rows and false for the rest.
    pub(crate) fn partition_point(&self, before: impl Fn(usize) -> bool) -> usize {
        let mut start = 0;
        let mut end = self.len();
        while start < end {
            let middle = start + (end - start) / 2;
            if before(self.at(middle)) {
                start = middle + 1;
            } else {
                end = middle;
            }
        }
        start
    }

    /// First position at or after `start` whose row fails `before`; `before` must hold for the
    /// rows from `start` up to that position.
    fn gallop(&self, start: usize, before: impl Fn(usize) -> bool) -> usize {
        gallop(self.len(), start, |position| before(self.at(position)))
    }
}

/// First position in `start..len` for which `before` fails; `before` is true from `start` up to
/// that position and false from there on. Exponential probing bounds the answer, the binary search
/// narrows it, so an answer near `start` costs few probes.
pub(crate) fn gallop(len: usize, start: usize, before: impl Fn(usize) -> bool) -> usize {
    debug_assert!(start <= len, "gallop starts outside the range");
    if start == len || !before(start) {
        return start;
    }
    let remaining = len - start;
    let mut lower = 1;
    let mut upper = 1;
    while upper < remaining && before(start + upper) {
        lower = upper + 1;
        upper = upper.saturating_mul(2);
    }
    let mut first = start + lower;
    let mut end = start + upper.min(remaining);
    while first < end {
        let middle = first + (end - first) / 2;
        if before(middle) {
            first = middle + 1;
        } else {
            end = middle;
        }
    }
    first
}

/// The rows of one relation ordered by one column: `rows` lists them, `values` holds every cell.
pub(crate) struct ColumnIndex<'a, C> {
    values: &'a [C],
    width: usize,
    column: usize,
    rows: RowIds<'a>,
}
impl<'a, C: Ord> ColumnIndex<'a, C> {
    pub(crate) fn new(values: &'a [C], width: usize, column: usize, rows: RowIds<'a>) -> Self {
        Self {
            values,
            width,
            column,
            rows,
        }
    }

    fn cell(&self, row: usize) -> &'a C {
        &self.values[row * self.width + self.column]
    }

    /// The rows whose value in this column is `value`.
    pub(crate) fn lookup(&self, value: &C) -> RowIds<'a> {
        let first = self.lower_bound(value);
        let rest = self.rows.slice(first..self.rows.len());
        let last = first + rest.partition_point(|row| self.cell(row) <= value);
        self.rows.slice(first..last)
    }

    /// The first row whose value in this column is not below `value`.
    pub(crate) fn lower_bound(&self, value: &C) -> usize {
        self.rows.partition_point(|row| self.cell(row) < value)
    }

    /// The rows whose value in this column is `value`, for callers asking for values in
    /// nondecreasing order.
    ///
    /// `cursor` retains the previous match. Every row before `cursor.end` holds a value below
    /// `value`, so the two searches resume there instead of at the start of the selection.
    pub(crate) fn matching(&self, cursor: &mut Range<usize>, value: &C) -> RowIds<'a> {
        if cursor.start < cursor.end && self.cell(self.rows.at(cursor.start)) == value {
            return self.rows.slice(cursor.clone());
        }
        let start = self.rows.gallop(cursor.end, |row| self.cell(row) < value);
        let end = self.rows.gallop(start, |row| self.cell(row) <= value);
        *cursor = start..end;
        self.rows.slice(start..end)
    }

    /// The largest key in the selection; only valid for a non-empty selection.
    pub(crate) fn last_key(&self) -> &'a C {
        self.cell(self.rows.at(self.rows.len() - 1))
    }
}
