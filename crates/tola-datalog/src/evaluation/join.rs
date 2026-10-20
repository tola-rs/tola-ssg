//! Resolving one body atom against a relation's rows.
//!
//! A positive literal keeps a binding per matching tuple, a negative one keeps the binding itself
//! when no tuple matches, so the two cannot share a result path: an empty lookup means opposite
//! things to them.
use super::clause::{Clause, MatchEvidence};
use super::program::Stage;
use super::tuples::RowBlock;
use super::{ConstantRef, ensure_not_cancelled};
use crate::index::{ColumnIndex, RowIds};
use crate::relation::RelationRows;
use crate::{Database, Error};
use smallvec::SmallVec;

pub(super) type Rows<'a, C> = RowBlock<ConstantRef<'a, C>>;

fn column_index<'a, C: Ord>(
    relation: &'a RelationRows<C>,
    column: usize,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<ColumnIndex<'a, C>, Error> {
    ensure_not_cancelled(is_cancelled)?;
    let index = relation.column(column);
    ensure_not_cancelled(is_cancelled)?;
    Ok(index)
}

/// The rows of the narrowest single indexed lookup among the demand's bound columns; the demand's
/// other bound columns are compared by the caller.
pub(super) fn candidate_row_ids<'a, C: Ord>(
    relation: &'a RelationRows<C>,
    columns: &[Option<ConstantRef<'_, C>>],
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<RowIds<'a>, Error> {
    let prefix = columns.iter().take_while(|value| value.is_some()).count();
    if prefix > 0 && columns[prefix..].iter().all(Option::is_none) {
        ensure_not_cancelled(is_cancelled)?;
        let rows = relation.matching_prefix(relation.row_ids(), 0..prefix, |column| {
            columns[column].unwrap().0
        });
        ensure_not_cancelled(is_cancelled)?;
        return Ok(rows);
    }
    let mut rows = relation.row_ids();
    for (column, value) in columns.iter().enumerate() {
        if let Some(value) = value {
            let index = column_index(relation, column, is_cancelled)?;
            let candidates = index.lookup(value.0);
            if candidates.is_empty() {
                return Ok(candidates);
            }
            if candidates.len() < rows.len() {
                rows = candidates;
            }
        }
    }
    Ok(rows)
}

pub(super) fn retain_negative<'a, C: Ord>(
    clause: &Clause<'a, C>,
    stage: &Stage,
    parameters: &[Option<ConstantRef<'a, C>>],
    bindings: Rows<'a, C>,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<Rows<'a, C>, Error> {
    let mut output = Rows::new(stage.width());
    for binding in bindings.iter() {
        ensure_not_cancelled(is_cancelled)?;
        // Every negative column is bound before the stage runs, so no tuple column is read.
        output.push(stage.values(clause, binding, parameters, &|_| unreachable!()));
    }
    Ok(output)
}

/// Rows of a base stage that matches no tuple: a negative atom keeps every binding, since no tuple
/// proves it, and a positive atom drops them.
fn no_match_rows<'a, C: Ord>(
    clause: &Clause<'a, C>,
    stage: &Stage,
    parameters: &[Option<ConstantRef<'a, C>>],
    bindings: Rows<'a, C>,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<Rows<'a, C>, Error> {
    if stage.negative {
        retain_negative(clause, stage, parameters, bindings, is_cancelled)
    } else {
        Ok(Rows::new(stage.width()))
    }
}

/// The rows the stage's constant columns select.
enum ConstantRun<'a> {
    /// No constant column narrows the candidates.
    Absent,
    /// A constant column selects no row, so the stage matches nothing.
    Empty,
    /// The shortest run and the column that selected it.
    Shortest { column: usize, rows: RowIds<'a> },
}

/// The relation a base stage scans, with the clause, stage and parameters every step reads.
struct BaseJoin<'a, 'b, C> {
    relation: &'a RelationRows<C>,
    clause: &'b Clause<'a, C>,
    stage: &'b Stage,
    parameters: &'b [Option<ConstantRef<'a, C>>],
}

impl<'a, 'b, C: Ord> BaseJoin<'a, 'b, C> {
    /// The value the stage has bound to `column` before reading a candidate tuple.
    fn bound(&self, binding: &[ConstantRef<'a, C>], column: usize) -> Option<ConstantRef<'a, C>> {
        self.stage
            .bound(self.clause, binding, self.parameters, column)
    }

    /// The join key of one binding; callers only ask for columns the stage has bound.
    fn key(&self, binding: &[ConstantRef<'a, C>], column: usize) -> &'a C {
        self.bound(binding, column).unwrap().0
    }

    /// The leading run of columns `first_binding` has bound, when every later column is unbound.
    ///
    /// `Some` bounds the stage with one prefix lookup: the columns before the join column are
    /// constants, so every binding shares them, and each binding rechecks the columns after it.
    /// Negative atoms stay on the general path, where a miss keeps their bindings instead of
    /// dropping them.
    fn fixed_prefix(&self, first_binding: &[ConstantRef<'a, C>]) -> Option<usize> {
        let prefix = (0..self.stage.arity())
            .take_while(|&column| self.bound(first_binding, column).is_some())
            .count();
        (!self.stage.negative
            && prefix > 0
            && (prefix..self.stage.arity())
                .all(|column| self.bound(first_binding, column).is_none()))
        .then_some(prefix)
    }

    /// The column each binding keys its join on: the first bound non-constant column, otherwise
    /// the first constant column, whose single value still indexes the run.
    fn join_column(&self, first_binding: &[ConstantRef<'a, C>]) -> Option<usize> {
        (0..self.stage.arity())
            .find(|&column| {
                !self.stage.is_constant_column(column)
                    && self.bound(first_binding, column).is_some()
            })
            .or_else(|| {
                (0..self.stage.arity()).find(|&column| self.stage.is_constant_column(column))
            })
    }

    /// The join key of every binding, indexed like `bindings`.
    fn keys(&self, bindings: &Rows<'a, C>, column: usize) -> SmallVec<[&'a C; 4]> {
        bindings
            .iter()
            .map(|binding| self.key(binding, column))
            .collect()
    }

    /// The constant column with the shortest run: every constant column must match, so the
    /// shortest run holds every candidate. `Empty` means one of them matches no row.
    fn constant_rows(
        &self,
        first_binding: &[ConstantRef<'a, C>],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<ConstantRun<'a>, Error> {
        let mut smallest: Option<(usize, RowIds<'a>)> = None;
        for column in 0..self.stage.arity() {
            if !self.stage.is_constant_column(column) {
                continue;
            }
            let value = self.bound(first_binding, column).unwrap();
            let rows = column_index(self.relation, column, is_cancelled)?.lookup(value.0);
            if rows.is_empty() {
                return Ok(ConstantRun::Empty);
            }
            if smallest
                .as_ref()
                .is_none_or(|(_, current)| rows.len() < current.len())
            {
                smallest = Some((column, rows));
            }
        }
        Ok(match smallest {
            Some((column, rows)) => ConstantRun::Shortest { column, rows },
            None => ConstantRun::Absent,
        })
    }

    /// The join column ordered over the rows the fixed prefix allows. `None` when that prefix
    /// matches no row, which only a positive stage can reach.
    fn join_index(
        &self,
        first_binding: &[ConstantRef<'a, C>],
        prefix: Option<usize>,
        column: usize,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Option<ColumnIndex<'a, C>>, Error> {
        if prefix.is_none() {
            return column_index(self.relation, column, is_cancelled).map(Some);
        }
        let rows = if column == 0 {
            self.relation.row_ids()
        } else {
            ensure_not_cancelled(is_cancelled)?;
            let rows =
                self.relation
                    .matching_prefix(self.relation.row_ids(), 0..column, |column| {
                        self.key(first_binding, column)
                    });
            ensure_not_cancelled(is_cancelled)?;
            if rows.is_empty() {
                return Ok(None);
            }
            rows
        };
        Ok(Some(self.relation.ordered_column(column, rows)))
    }

    /// Narrows candidates by the fixed prefix columns after the join column, which the index
    /// lookup does not check.
    fn narrow_prefix(
        &self,
        candidates: RowIds<'a>,
        prefix: usize,
        join_column: usize,
        binding: &[ConstantRef<'a, C>],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<RowIds<'a>, Error> {
        if join_column + 1 >= prefix || candidates.is_empty() {
            return Ok(candidates);
        }
        let rows = self
            .relation
            .matching_prefix(candidates, join_column + 1..prefix, |column| {
                self.key(binding, column)
            });
        ensure_not_cancelled(is_cancelled)?;
        Ok(rows)
    }

    /// Appends what one binding produces: one row per matching tuple, or the binding itself when a
    /// negative atom has no matching tuple.
    fn emit_binding(
        &self,
        binding: &[ConstantRef<'a, C>],
        candidates: &RowIds<'a>,
        evidence: MatchEvidence,
        output: &mut Rows<'a, C>,
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<(), Error> {
        let matches_all = self.stage.matches_all(self.parameters, evidence);
        if matches_all && !self.stage.negative {
            ensure_not_cancelled(is_cancelled)?;
            output.reserve_rows(candidates.len());
        }
        output.append_run(self.stage.negative || self.stage.preserves_order, |run| {
            let mut present = false;
            for row in candidates.iter() {
                ensure_not_cancelled(is_cancelled)?;
                let tuple = self.relation.row(row);
                let value = |column| ConstantRef(&tuple[column]);
                if matches_all
                    || self
                        .stage
                        .matches(self.clause, binding, self.parameters, evidence, &value)
                {
                    if self.stage.negative {
                        present = true;
                        break;
                    }
                    run.push(
                        self.stage
                            .values(self.clause, binding, self.parameters, &value),
                    );
                }
            }
            if self.stage.negative && !present {
                run.push(self.stage.values(
                    self.clause,
                    binding,
                    self.parameters,
                    &|_| unreachable!(),
                ));
            }
            Ok(())
        })
    }
}

/// Whether the smallest join key lies past the last key the index holds, so no row can match.
fn keys_past_index<C: Ord>(
    stage: &Stage,
    column: usize,
    keys: &[&C],
    strictly_sorted: bool,
    index: &ColumnIndex<'_, C>,
) -> bool {
    // A fixed column, or column 0 of a strictly sorted driver, makes the first key minimal.
    let smallest = if stage.is_fixed_column(column)
        || (stage.input_column(column) == Some(0) && strictly_sorted)
    {
        keys[0]
    } else {
        keys.iter().copied().min().unwrap()
    };
    smallest > index.last_key()
}

/// Produces the rows one base atom contributes to `bindings`: joins the stage against the stored
/// relation, with negative atoms keeping the bindings that have no match.
pub(super) fn join_base<'a, C: Ord>(
    database: &'a Database<C>,
    is_cancelled: &mut impl FnMut() -> bool,
    clause: &Clause<'a, C>,
    stage: &Stage,
    parameters: &[Option<ConstantRef<'a, C>>],
    bindings: Rows<'a, C>,
) -> Result<Rows<'a, C>, Error> {
    let Some(relation) = database.snapshot.relations.get(clause.relation(stage)) else {
        return no_match_rows(clause, stage, parameters, bindings, is_cancelled);
    };
    debug_assert!(
        !bindings.is_empty(),
        "join_base requires at least one binding"
    );
    let first_binding = bindings.row(0);
    let join = BaseJoin {
        relation,
        clause,
        stage,
        parameters,
    };
    let prefix = join.fixed_prefix(first_binding);
    let column = join.join_column(first_binding);
    // A fixed prefix already bounds the run; constant columns only help on the general path.
    let constant_rows = match prefix {
        Some(_) => ConstantRun::Absent,
        None => join.constant_rows(first_binding, is_cancelled)?,
    };
    if matches!(constant_rows, ConstantRun::Empty) {
        return no_match_rows(clause, stage, parameters, bindings, is_cancelled);
    }
    let keys = column.map(|column| join.keys(&bindings, column));
    let join_index = match column {
        Some(column) => match join.join_index(first_binding, prefix, column, is_cancelled)? {
            Some(index) => Some((column, index)),
            None => return no_match_rows(clause, stage, parameters, bindings, is_cancelled),
        },
        None => None,
    };
    if let (Some((column, index)), Some(keys)) = (&join_index, &keys)
        && keys_past_index(stage, *column, keys, bindings.is_strictly_sorted(), index)
    {
        return no_match_rows(clause, stage, parameters, bindings, is_cancelled);
    }
    let order = keys.as_deref().filter(|keys| keys.len() > 1).map(|keys| {
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_unstable_by(|&left, &right| keys[left].cmp(keys[right]));
        order
    });
    // Start at the lowest key so the first lookup does not gallop from row zero.
    let mut cursor = match (&join_index, &keys) {
        (Some((_, index)), Some(keys)) => {
            let row = order.as_ref().map_or(0, |order| order[0]);
            let first = index.lower_bound(keys[row]);
            first..first
        }
        _ => 0..0,
    };
    let mut output = Rows::new(stage.width());
    for position in 0..bindings.len() {
        let binding_index = order.as_ref().map_or(position, |order| order[position]);
        let binding = bindings.row(binding_index);
        ensure_not_cancelled(is_cancelled)?;
        let candidates = match (&join_index, &keys) {
            (Some((_, index)), Some(keys)) => index.matching(&mut cursor, keys[binding_index]),
            _ => relation.row_ids(),
        };
        let candidates = match (prefix, &join_index) {
            (Some(prefix), Some((column, _))) => {
                join.narrow_prefix(candidates, prefix, *column, binding, is_cancelled)?
            }
            _ => candidates,
        };
        let (candidates, evidence) = match &constant_rows {
            ConstantRun::Shortest {
                column: constant_column,
                rows,
            } if rows.len() < candidates.len() => (rows, MatchEvidence::Column(*constant_column)),
            _ => (
                &candidates,
                column.map_or(MatchEvidence::None, MatchEvidence::Column),
            ),
        };
        join.emit_binding(binding, candidates, evidence, &mut output, is_cancelled)?;
    }
    Ok(output)
}
