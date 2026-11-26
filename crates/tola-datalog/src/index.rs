//! Row selections and exponential-probing column indexes.

use std::ops::Range;

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

    fn at(&self, pos: usize) -> usize {
        match self {
            Self::Range(rows) => rows.start + pos,
            Self::Indexed(rows) => rows[pos],
        }
    }

    pub(crate) fn slice(&self, pos: Range<usize>) -> Self {
        debug_assert!(pos.end <= self.len(), "slice outside the selection");
        match self {
            Self::Range(rows) => Self::Range(rows.start + pos.start..rows.start + pos.end),
            Self::Indexed(rows) => {
                let rows = *rows;
                Self::Indexed(&rows[pos])
            }
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len()).map(|pos| self.at(pos))
    }

    pub(crate) fn partition_point(&self, before: impl Fn(usize) -> bool) -> usize {
        let mut lo = 0;
        let mut hi = self.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if before(self.at(mid)) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub(crate) fn gallop(&self, start: usize, before: impl Fn(usize) -> bool) -> usize {
        gallop(self.len(), start, |pos| before(self.at(pos)))
    }
}

pub(crate) fn gallop(len: usize, start: usize, before: impl Fn(usize) -> bool) -> usize {
    debug_assert!(start <= len, "gallop starts outside the range");
    if start == len || !before(start) {
        return start;
    }
    let rem = len - start;
    let mut lo = 1;
    let mut hi = 1;
    while hi < rem && before(start + hi) {
        lo = hi + 1;
        hi = hi.saturating_mul(2);
    }
    let mut first = start + lo;
    let mut end = start + hi.min(rem);
    while first < end {
        let mid = first + (end - first) / 2;
        if before(mid) {
            first = mid + 1;
        } else {
            end = mid;
        }
    }
    first
}

pub(crate) struct ColumnIndex<'a, C> {
    vals: &'a [C],
    width: usize,
    col: usize,
    rows: RowIds<'a>,
}

impl<'a, C: Ord> ColumnIndex<'a, C> {
    pub(crate) fn new(vals: &'a [C], width: usize, col: usize, rows: RowIds<'a>) -> Self {
        Self {
            vals,
            width,
            col,
            rows,
        }
    }

    fn cell(&self, row: usize) -> &'a C {
        &self.vals[row * self.width + self.col]
    }

    pub(crate) fn lookup(&self, val: &C) -> RowIds<'a> {
        let first = self.lower_bound(val);
        let last = self.rows.gallop(first, |row| self.cell(row) <= val);
        self.rows.slice(first..last)
    }

    pub(crate) fn lower_bound(&self, val: &C) -> usize {
        self.rows.partition_point(|row| self.cell(row) < val)
    }

    /// Match `val` assuming monotonically nondecreasing probes tracked via `cursor`.
    pub(crate) fn matching(&self, cursor: &mut Range<usize>, val: &C) -> RowIds<'a> {
        if cursor.start < cursor.end && self.cell(self.rows.at(cursor.start)) == val {
            return self.rows.slice(cursor.clone());
        }
        let start = self.rows.gallop(cursor.end, |row| self.cell(row) < val);
        let end = self.rows.gallop(start, |row| self.cell(row) <= val);
        *cursor = start..end;
        self.rows.slice(start..end)
    }

    pub(crate) fn last_key(&self) -> &'a C {
        self.cell(self.rows.at(self.rows.len() - 1))
    }
}
