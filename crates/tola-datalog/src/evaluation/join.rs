//! Base-relation join and projection for one body atom.

use super::clause::{Clause, ClauseMatch};
use super::program::Stage;
use super::tuples::RowBlock;
use super::{ConstantRef, ensure_not_cancelled};
use crate::index::{ColumnIndex, RowIds};
use crate::relation::RelationRows;
use crate::{Database, Error};

pub(super) type Rows<'a, C> = RowBlock<ConstantRef<'a, C>>;

fn column_index<'a, C: Ord>(
    rel: &'a RelationRows<C>,
    col: usize,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<ColumnIndex<'a, C>, Error> {
    ensure_not_cancelled(is_cancelled)?;
    let idx = rel.column(col);
    ensure_not_cancelled(is_cancelled)?;
    Ok(idx)
}

pub(super) fn candidate_row_ids<'a, C: Ord>(
    rel: &'a RelationRows<C>,
    cols: &[Option<ConstantRef<'_, C>>],
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<(RowIds<'a>, ClauseMatch), Error> {
    let prefix = cols.iter().take_while(|v| v.is_some()).count();
    if prefix > 0 && cols[prefix..].iter().all(Option::is_none) {
        ensure_not_cancelled(is_cancelled)?;
        let rows = rel.matching_prefix(rel.row_ids(), 0..prefix, |c| cols[c].unwrap().0);
        ensure_not_cancelled(is_cancelled)?;
        return Ok((rows, ClauseMatch::Prefix(prefix)));
    }
    let mut rows = rel.row_ids();
    let mut ev = ClauseMatch::None;
    for (col, val) in cols.iter().enumerate() {
        if let Some(val) = val {
            let idx = column_index(rel, col, is_cancelled)?;
            let cands = idx.lookup(val.0);
            if cands.is_empty() {
                return Ok((cands, ClauseMatch::Column(col)));
            }
            if cands.len() < rows.len() {
                rows = cands;
                ev = ClauseMatch::Column(col);
            }
        }
    }
    Ok((rows, ev))
}

pub(super) fn project_bindings<'a, C: Ord>(
    clause: &Clause<'a, C>,
    stage: &Stage,
    params: &[Option<ConstantRef<'a, C>>],
    bindings: &Rows<'a, C>,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<Rows<'a, C>, Error> {
    let mut out = Rows::new(stage.width());
    for binding in bindings.iter() {
        ensure_not_cancelled(is_cancelled)?;
        out.push(stage.values(clause, binding, params, &|_| unreachable!()));
        if stage.width() == 0 {
            break;
        }
    }
    Ok(out)
}

fn no_match_rows<'a, C: Ord>(
    clause: &Clause<'a, C>,
    stage: &Stage,
    params: &[Option<ConstantRef<'a, C>>],
    bindings: &Rows<'a, C>,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<Rows<'a, C>, Error> {
    if stage.negative {
        project_bindings(clause, stage, params, bindings, is_cancelled)
    } else {
        Ok(Rows::new(stage.width()))
    }
}

enum ConstantRun<'a> {
    Absent,
    Empty,
    Shortest { col: usize, rows: RowIds<'a> },
}

struct BaseJoin<'a, 'b, C> {
    rel: &'a RelationRows<C>,
    clause: &'b Clause<'a, C>,
    stage: &'b Stage,
    params: &'b [Option<ConstantRef<'a, C>>],
    reads_tup: bool,
}

impl<'a, 'b, C: Ord> BaseJoin<'a, 'b, C> {
    fn bound(&self, binding: &[ConstantRef<'a, C>], col: usize) -> Option<ConstantRef<'a, C>> {
        self.stage.bound(self.clause, binding, self.params, col)
    }

    fn key(&self, binding: &[ConstantRef<'a, C>], col: usize) -> &'a C {
        self.bound(binding, col).unwrap().0
    }

    fn binding_order(
        &self,
        bindings: &Rows<'a, C>,
        col: usize,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Option<Vec<usize>>, Error> {
        let Some(input) = self.stage.input_column(col) else {
            return Ok(None);
        };
        if bindings.len() <= 1 || (input == 0 && bindings.is_strictly_sorted()) {
            return Ok(None);
        }
        ensure_not_cancelled(is_cancelled)?;
        let mut order: Vec<usize> = (0..bindings.len()).collect();
        // Preserve arrival order on equal join keys so projected duplicates keep their first source.
        order.sort_unstable_by(|&l, &r| {
            bindings.row(l)[input]
                .cmp(&bindings.row(r)[input])
                .then_with(|| l.cmp(&r))
        });
        ensure_not_cancelled(is_cancelled)?;
        Ok(Some(order))
    }

    fn fixed_prefix(&self, first: &[ConstantRef<'a, C>]) -> Option<usize> {
        let prefix = (0..self.stage.arity())
            .take_while(|&c| self.bound(first, c).is_some())
            .count();
        (prefix > 0 && (prefix..self.stage.arity()).all(|c| self.bound(first, c).is_none()))
            .then_some(prefix)
    }

    fn join_column(&self, first: &[ConstantRef<'a, C>]) -> Option<usize> {
        (0..self.stage.arity())
            .find(|&c| !self.stage.is_constant_column(c) && self.bound(first, c).is_some())
            .or_else(|| (0..self.stage.arity()).find(|&c| self.stage.is_constant_column(c)))
    }

    fn constant_rows(
        &self,
        first: &[ConstantRef<'a, C>],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<ConstantRun<'a>, Error> {
        let mut smallest: Option<(usize, RowIds<'a>)> = None;
        for col in 0..self.stage.arity() {
            if !self.stage.is_constant_column(col) {
                continue;
            }
            let val = self.bound(first, col).unwrap();
            let rows = column_index(self.rel, col, is_cancelled)?.lookup(val.0);
            if rows.is_empty() {
                return Ok(ConstantRun::Empty);
            }
            if smallest
                .as_ref()
                .is_none_or(|(_, cur)| rows.len() < cur.len())
            {
                smallest = Some((col, rows));
            }
        }
        Ok(match smallest {
            Some((col, rows)) => ConstantRun::Shortest { col, rows },
            None => ConstantRun::Absent,
        })
    }

    fn join_index(
        &self,
        first: &[ConstantRef<'a, C>],
        prefix: Option<usize>,
        col: usize,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Option<ColumnIndex<'a, C>>, Error> {
        if prefix.is_none() {
            return column_index(self.rel, col, is_cancelled).map(Some);
        }
        let rows = if col == 0 {
            self.rel.row_ids()
        } else {
            ensure_not_cancelled(is_cancelled)?;
            let rows = self
                .rel
                .matching_prefix(self.rel.row_ids(), 0..col, |c| self.key(first, c));
            ensure_not_cancelled(is_cancelled)?;
            if rows.is_empty() {
                return Ok(None);
            }
            rows
        };
        Ok(Some(self.rel.ordered_column(col, rows)))
    }

    fn narrow_prefix(
        &self,
        cands: RowIds<'a>,
        prefix: usize,
        join_col: usize,
        binding: &[ConstantRef<'a, C>],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<RowIds<'a>, Error> {
        if join_col + 1 >= prefix || cands.is_empty() {
            return Ok(cands);
        }
        let rows = self
            .rel
            .matching_prefix(cands, join_col + 1..prefix, |c| self.key(binding, c));
        ensure_not_cancelled(is_cancelled)?;
        Ok(rows)
    }

    fn emit_binding(
        &self,
        binding: &[ConstantRef<'a, C>],
        cands: &RowIds<'a>,
        ev: ClauseMatch,
        out: &mut Rows<'a, C>,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        let matches_all = self.stage.matches_all(self.params, ev);
        out.append_run(self.stage.negative || self.stage.preserves_order, |run| {
            if matches_all && !self.reads_tup {
                if cands.is_empty() == self.stage.negative {
                    run.push(self.stage.values(
                        self.clause,
                        binding,
                        self.params,
                        &|_| unreachable!(),
                    ));
                }
                return Ok(());
            }
            let mut present = false;
            for row in cands.iter() {
                ensure_not_cancelled(is_cancelled)?;
                let tup = self.rel.row(row);
                let val = |col| ConstantRef(&tup[col]);
                if matches_all
                    || self
                        .stage
                        .matches(self.clause, binding, self.params, ev, &val)
                {
                    if self.stage.negative {
                        present = true;
                        break;
                    }
                    run.push(self.stage.values(self.clause, binding, self.params, &val));
                    if !self.reads_tup {
                        break;
                    }
                }
            }
            if self.stage.negative && !present {
                run.push(
                    self.stage
                        .values(self.clause, binding, self.params, &|_| unreachable!()),
                );
            }
            Ok(())
        })
    }
}

pub(super) fn join_base<'a, C: Ord>(
    db: &'a Database<C>,
    is_cancelled: &mut impl FnMut() -> bool,
    clause: &Clause<'a, C>,
    stage: &Stage,
    params: &[Option<ConstantRef<'a, C>>],
    bindings: &Rows<'a, C>,
) -> Result<Rows<'a, C>, Error> {
    let Some(rel) = db.snapshot.relations.get(clause.relation(stage)) else {
        return no_match_rows(clause, stage, params, bindings, is_cancelled);
    };
    debug_assert!(!bindings.is_empty(), "join_base requires >= 1 binding");
    let first_binding = bindings.row(0);
    let join = BaseJoin {
        rel,
        clause,
        stage,
        params,
        reads_tup: stage.reads_tuple(params),
    };
    let prefix = join.fixed_prefix(first_binding);
    let col = join.join_column(first_binding);
    let const_rows = match prefix {
        Some(_) => ConstantRun::Absent,
        None => join.constant_rows(first_binding, is_cancelled)?,
    };
    if matches!(const_rows, ConstantRun::Empty) {
        return no_match_rows(clause, stage, params, bindings, is_cancelled);
    }
    let join_idx = match col {
        Some(c) => match join.join_index(first_binding, prefix, c, is_cancelled)? {
            Some(idx) => Some((c, idx)),
            None => return no_match_rows(clause, stage, params, bindings, is_cancelled),
        },
        None => None,
    };
    let (order, mut cursor) = if let Some((c, idx)) = &join_idx {
        let last = idx.last_key();
        let mut can_match = join.key(first_binding, *c) <= last;
        if !can_match
            && let Some(input) = stage.input_column(*c)
            && (input != 0 || !bindings.is_strictly_sorted())
        {
            for binding in bindings.iter().skip(1) {
                ensure_not_cancelled(is_cancelled)?;
                if join.key(binding, *c) <= last {
                    can_match = true;
                    break;
                }
            }
        }
        if !can_match {
            return no_match_rows(clause, stage, params, bindings, is_cancelled);
        }
        let order = join.binding_order(bindings, *c, is_cancelled)?;
        let first_idx = order.as_ref().map_or(0, |o| o[0]);
        let first_key = join.key(bindings.row(first_idx), *c);
        let first_pos = idx.lower_bound(first_key);
        (order, first_pos..first_pos)
    } else {
        (None, 0..0)
    };
    let indexed_ev = match (prefix, col) {
        (Some(p), _) => ClauseMatch::Prefix(p),
        (_, Some(c)) => ClauseMatch::Column(c),
        _ => ClauseMatch::None,
    };
    let mut out = Rows::new(stage.width());
    for pos in 0..bindings.len() {
        let binding_idx = order.as_ref().map_or(pos, |o| o[pos]);
        let binding = bindings.row(binding_idx);
        ensure_not_cancelled(is_cancelled)?;
        let cands = match &join_idx {
            Some((c, idx)) => idx.matching(&mut cursor, join.key(binding, *c)),
            None => rel.row_ids(),
        };
        let cands = match (prefix, &join_idx) {
            (Some(p), Some((c, _))) => join.narrow_prefix(cands, p, *c, binding, is_cancelled)?,
            _ => cands,
        };
        let (cands, ev) = match &const_rows {
            ConstantRun::Shortest { col: cc, rows } if rows.len() < cands.len() => {
                (rows, ClauseMatch::Column(*cc))
            }
            _ => (&cands, indexed_ev),
        };
        join.emit_binding(binding, cands, ev, &mut out, is_cancelled)?;
        if stage.width() == 0 && !out.is_empty() {
            break;
        }
    }
    Ok(out)
}
