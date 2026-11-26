//! Demand-driven tabled evaluation of rules and queries.

mod clause;
mod join;
mod program;
mod tuples;

use self::clause::{Clause, ClauseMatch};
use self::join::{Rows, candidate_row_ids, join_base, project_bindings};
use self::program::Stage;
pub(crate) use self::program::{Program, RuleProgram};
use self::tuples::RowBlock;
use crate::validation::{atom, bound_variables, validate_body};
use crate::{Database, Error, ErrorKind, Location, Query};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

/// Borrowed constant that checks pointer equality first to skip value comparisons in joins.
struct ConstantRef<'a, C>(&'a C);

impl<C> Copy for ConstantRef<'_, C> {}
impl<C> Clone for ConstantRef<'_, C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C: Eq> PartialEq for ConstantRef<'_, C> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0) || self.0 == other.0
    }
}
impl<C: Eq> Eq for ConstantRef<'_, C> {}
impl<C: Ord> PartialOrd for ConstantRef<'_, C> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<C: Ord> Ord for ConstantRef<'_, C> {
    fn cmp(&self, other: &Self) -> Ordering {
        if std::ptr::eq(self.0, other.0) {
            Ordering::Equal
        } else {
            self.0.cmp(other.0)
        }
    }
}

fn ensure_not_cancelled(is_cancelled: &mut impl FnMut() -> bool) -> Result<(), Error> {
    if is_cancelled() {
        Err(Error {
            location: Location::Database,
            kind: ErrorKind::Cancelled,
        })
    } else {
        Ok(())
    }
}

impl<C: Clone + Ord> Database<C> {
    /// Evaluate `query` over this snapshot, returning sorted distinct rows or an error if cancelled.
    pub fn query(
        &self,
        query: &Query<C>,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<Vec<C>>, Error> {
        let bound = bound_variables(&query.body);
        validate_body(
            &query.body,
            &bound,
            &self.snapshot.arities,
            Location::QueryBody,
        )?;
        for (idx, var) in query.select.iter().enumerate() {
            if var.is_empty() {
                return Err(Error {
                    location: Location::QuerySelect(idx),
                    kind: ErrorKind::EmptyVariable,
                });
            }
            if !bound.contains(var) {
                return Err(Error {
                    location: Location::QuerySelect(idx),
                    kind: ErrorKind::UnboundVariable(var.clone()),
                });
            }
        }
        let program = Program::query(query);
        let level = query
            .body
            .iter()
            .map(|lit| self.snapshot.strata[&atom(lit).relation.name])
            .max()
            .unwrap_or(0)
            + 1;
        let mut eval = Evaluator::new(self, &mut is_cancelled, level, query.select.len());
        let clause = Rc::new(Clause::query(query, &program));
        eval.enqueue(
            level,
            Work::Advance(Continuation {
                rows: Rows::single_row(Vec::new()),
                clause,
                params: Rc::new(Vec::new()),
                pos: 0,
                dst: Dst::Query,
            }),
        );
        eval.complete()?;
        eval.rows.sort_distinct(eval.is_cancelled)?;
        let mut out = Vec::with_capacity(eval.rows.len());
        for row in eval.rows.iter() {
            ensure_not_cancelled(eval.is_cancelled)?;
            out.push(row.iter().map(|v| v.0.clone()).collect());
        }
        ensure_not_cancelled(eval.is_cancelled)?;
        Ok(out)
    }
}

type BoundCols<'a, C> = SmallVec<[Option<ConstantRef<'a, C>>; 2]>;
type GroundTuple<'a, C> = SmallVec<[ConstantRef<'a, C>; 2]>;
type DemandGroups<'a, C> = BTreeMap<BoundCols<'a, C>, Rows<'a, C>>;

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct Demand<'a, C: Ord> {
    rel: &'a str,
    cols: BoundCols<'a, C>,
}

#[derive(Clone)]
enum Dst {
    Table(usize),
    Query,
}

struct Continuation<'a, C> {
    clause: Rc<Clause<'a, C>>,
    params: Rc<Vec<Option<ConstantRef<'a, C>>>>,
    pos: usize,
    rows: Rows<'a, C>,
    dst: Dst,
}

impl<'a, C> Continuation<'a, C> {
    fn is_query_output(&self) -> bool {
        matches!(self.dst, Dst::Query) && self.pos == self.clause.len()
    }

    fn with_rows(&self, rows: Rows<'a, C>) -> Self {
        Self {
            clause: self.clause.clone(),
            params: self.params.clone(),
            pos: self.pos,
            rows,
            dst: self.dst.clone(),
        }
    }
}

struct Consumer<'a, C> {
    cont: Continuation<'a, C>,
    stratum: usize,
    reads_tup: bool,
}

impl<'a, C: Ord> Consumer<'a, C> {
    fn resume(
        &self,
        batches: &[Rows<'a, C>],
        is_cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Option<Continuation<'a, C>>, Error> {
        if batches.is_empty() {
            return Ok(None);
        }
        let clause = &self.cont.clause;
        let params = &self.cont.params;
        let stage = clause.stage(self.cont.pos - 1);
        let ev = ClauseMatch::Demand;
        let matches_all = stage.matches_all(params, ev);
        if !self.reads_tup {
            // Every binding in this demand shares the same bound columns, so the first match resumes all.
            let bindings = &self.cont.rows;
            for tup in batches.iter().flat_map(RowBlock::iter) {
                ensure_not_cancelled(is_cancelled)?;
                let val = |col| tup[col];
                if matches_all || stage.matches(clause, bindings.row(0), params, ev, &val) {
                    let rows = project_bindings(clause, stage, params, bindings, is_cancelled)?;
                    return Ok(Some(self.cont.with_rows(rows)));
                }
            }
            return Ok(None);
        }
        let mut rows = Rows::new(stage.width());
        let identity = matches_all && stage.identity(params);
        for bindings in self.cont.rows.iter() {
            for batch in batches {
                rows.append_run(
                    stage.preserves_order && batch.is_strictly_sorted(),
                    |rows| {
                        if identity {
                            rows.append_block(batch);
                            ensure_not_cancelled(is_cancelled)?;
                        } else {
                            for tup in batch.iter() {
                                ensure_not_cancelled(is_cancelled)?;
                                let val = |col| tup[col];
                                if matches_all || stage.matches(clause, bindings, params, ev, &val)
                                {
                                    rows.push(stage.values(clause, bindings, params, &val));
                                }
                            }
                        }
                        Ok(())
                    },
                )?;
            }
        }
        Ok((!rows.is_empty()).then(|| self.cont.with_rows(rows)))
    }
}

struct Table<'a, C> {
    answers: Vec<Rows<'a, C>>,
    consumers: Vec<Consumer<'a, C>>,
}

struct NegativeBatch<'a, C: Ord> {
    cover: Demand<'a, C>,
    cont: Continuation<'a, C>,
}

enum Work<'a, C: Ord> {
    Advance(Continuation<'a, C>),
    Answer(usize, Rows<'a, C>),
    CheckNegative(usize, Continuation<'a, C>),
    CheckNegativeBatch(Box<NegativeBatch<'a, C>>),
}

fn push_continuation<'a, C: Ord>(queue: &mut VecDeque<Work<'a, C>>, next: Continuation<'a, C>) {
    if next.is_query_output()
        && let Some(Work::Advance(prev)) = queue.back_mut()
        && prev.is_query_output()
        && prev.rows.has_capacity_for(&next.rows)
    {
        prev.rows.extend(next.rows);
    } else {
        queue.push_back(Work::Advance(next));
    }
}

struct Evaluator<'a, C: Ord, F> {
    db: &'a Database<C>,
    is_cancelled: &'a mut F,
    rule_clauses: BTreeMap<usize, Rc<Clause<'a, C>>>,
    table_ids: BTreeMap<Demand<'a, C>, usize>,
    tables: Vec<Table<'a, C>>,
    queues: Vec<VecDeque<Work<'a, C>>>,
    level: usize,
    rows: Rows<'a, C>,
}

fn group_demands<'a, C: Ord>(
    cont: &Continuation<'a, C>,
    stage: &Stage,
    rows: &Rows<'a, C>,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<DemandGroups<'a, C>, Error> {
    let mut cols = BoundCols::new();
    let mut groups = DemandGroups::new();
    for row in rows.iter() {
        ensure_not_cancelled(is_cancelled)?;
        cols.clear();
        cols.extend(demand_cols(cont, stage, row));
        if let Some(bindings) = groups.get_mut(cols.as_slice()) {
            bindings.push(row.iter().copied());
            continue;
        }
        let mut bindings = Rows::new(rows.width());
        bindings.push(row.iter().copied());
        groups.insert(cols.clone(), bindings);
    }
    Ok(groups)
}

fn demand_cols<'a, 'b, C>(
    cont: &'b Continuation<'a, C>,
    stage: &'b Stage,
    row: &'b [ConstantRef<'a, C>],
) -> impl ExactSizeIterator<Item = Option<ConstantRef<'a, C>>> + 'b {
    (0..stage.arity()).map(move |col| stage.bound(&cont.clause, row, &cont.params, col))
}

impl<'a, C: Clone + Ord, F: FnMut() -> bool> Evaluator<'a, C, F> {
    fn new(
        db: &'a Database<C>,
        is_cancelled: &'a mut F,
        query_level: usize,
        out_width: usize,
    ) -> Self {
        Self {
            db,
            is_cancelled,
            rule_clauses: BTreeMap::new(),
            table_ids: BTreeMap::new(),
            tables: Vec::new(),
            // Rules only depend on <= their own stratum, so `query_level` bounds the queues.
            queues: (0..=query_level).map(|_| VecDeque::new()).collect(),
            level: query_level,
            rows: RowBlock::new(out_width),
        }
    }

    fn enqueue(&mut self, level: usize, work: Work<'a, C>) {
        self.queues[level].push_back(work);
        self.level = self.level.min(level);
    }

    fn stratum(&self, name: &str) -> usize {
        self.db.snapshot.strata[name]
    }

    fn ensure_table(&mut self, demand: Demand<'a, C>) -> Result<usize, Error> {
        if let Some(&id) = self.table_ids.get(&demand) {
            return Ok(id);
        }
        let id = self.tables.len();
        let level = self.stratum(demand.rel);
        self.tables.push(Table {
            answers: Vec::new(),
            consumers: Vec::new(),
        });
        self.seed_base_answers(id, &demand, level)?;
        self.start_rule_consumers(id, &demand, level)?;
        self.table_ids.insert(demand, id);
        Ok(id)
    }

    fn seed_base_answers(
        &mut self,
        table: usize,
        demand: &Demand<'a, C>,
        level: usize,
    ) -> Result<(), Error> {
        let mut tups = RowBlock::new(demand.cols.len());
        if let Some(rel) = self.db.snapshot.relations.get(demand.rel) {
            let (cands, matched) = candidate_row_ids(rel, &demand.cols, self.is_cancelled)?;
            let rem: SmallVec<[_; 4]> = demand
                .cols
                .iter()
                .enumerate()
                .filter_map(|(col, &val)| val.filter(|_| !matched.includes(col)).map(|v| (col, v)))
                .collect();
            tups.append_run(true, |tups| {
                for row in cands.iter() {
                    ensure_not_cancelled(self.is_cancelled)?;
                    let tup = rel.row(row);
                    if rem
                        .iter()
                        .all(|&(col, bound)| bound == ConstantRef(&tup[col]))
                    {
                        tups.push(tup.iter().map(ConstantRef));
                    }
                }
                Ok(())
            })?;
        }
        if !tups.is_empty() {
            self.enqueue(level, Work::Answer(table, tups));
        }
        Ok(())
    }

    fn start_rule_consumers(
        &mut self,
        table: usize,
        demand: &Demand<'a, C>,
        level: usize,
    ) -> Result<(), Error> {
        for &idx in self
            .db
            .snapshot
            .rules_by_head
            .get(demand.rel)
            .into_iter()
            .flatten()
        {
            ensure_not_cancelled(self.is_cancelled)?;
            let clause = self
                .rule_clauses
                .entry(idx)
                .or_insert_with(|| Rc::new(Clause::rule(&self.db.snapshot.rules[idx])))
                .clone();
            let Some(params) = clause.parameters(demand.cols.iter().copied()) else {
                continue;
            };
            self.enqueue(
                level,
                Work::Advance(Continuation {
                    clause,
                    params: Rc::new(params),
                    pos: 0,
                    rows: Rows::single_row(Vec::new()),
                    dst: Dst::Table(table),
                }),
            );
        }
        Ok(())
    }

    fn complete(&mut self) -> Result<(), Error> {
        // Lower strata drain completely before any negative check observes their tables.
        while let Some((level, work)) = self.next_work() {
            ensure_not_cancelled(self.is_cancelled)?;
            match work {
                Work::Answer(table, tups) => self.accept_answers(table, tups),
                Work::CheckNegative(table, cont) => self.check_negative(level, table, cont),
                Work::CheckNegativeBatch(batch) => self.check_negative_batch(level, *batch),
                Work::Advance(cont) => self.advance(level, cont),
            }?;
        }
        Ok(())
    }

    fn next_work(&mut self) -> Option<(usize, Work<'a, C>)> {
        while self.level < self.queues.len() {
            let level = self.level;
            if let Some(work) = self.queues[level].pop_front() {
                return Some((level, work));
            }
            self.level += 1;
        }
        None
    }

    fn accept_answers(&mut self, table: usize, mut tups: Rows<'a, C>) -> Result<(), Error> {
        let table = &mut self.tables[table];
        tups.retain_unseen(&table.answers, self.is_cancelled)?;
        if tups.is_empty() {
            return Ok(());
        }
        let mut waiting = 0;
        for idx in 0..table.consumers.len() {
            let consumer = &table.consumers[idx];
            if let Some(next) = consumer.resume(std::slice::from_ref(&tups), self.is_cancelled)? {
                push_continuation(&mut self.queues[consumer.stratum], next);
                self.level = self.level.min(consumer.stratum);
                if !consumer.reads_tup {
                    continue;
                }
            }
            if waiting != idx {
                table.consumers.swap(waiting, idx);
            }
            waiting += 1;
        }
        table.consumers.truncate(waiting);
        // Keep answer batch sizes geometrically increasing toward the tail for O(log N) merges.
        while table
            .answers
            .last()
            .is_some_and(|old| old.len() <= tups.len() * 2)
        {
            tups = table
                .answers
                .pop()
                .unwrap()
                .merge_disjoint(tups, self.is_cancelled)?;
        }
        table.answers.push(tups);
        Ok(())
    }

    fn check_negative(
        &mut self,
        level: usize,
        table: usize,
        cont: Continuation<'a, C>,
    ) -> Result<(), Error> {
        if self.tables[table].answers.is_empty() {
            self.resume_negative(level, cont)?;
        }
        Ok(())
    }

    fn check_negative_batch(
        &mut self,
        level: usize,
        batch: NegativeBatch<'a, C>,
    ) -> Result<(), Error> {
        let NegativeBatch { cover, mut cont } = batch;
        debug_assert!(self.stratum(cover.rel) < level);
        let stage = cont.clause.stage(cont.pos - 1);
        let Some(&table) = self.table_ids.get(&cover) else {
            let groups = group_demands(&cont, stage, &cont.rows, self.is_cancelled)?;
            for (cols, rows) in groups {
                self.dispatch_demand(level, cols, cont.with_rows(rows))?;
            }
            return Ok(());
        };
        // Lower stratum has drained, so `cover` answers cover every ground tuple in the batch.
        let answers = &self.tables[table].answers;
        let mut cursors: SmallVec<[usize; 8]> = answers.iter().map(|_| 0).collect();
        let mut out = Rows::new(stage.width());
        let mut tup = GroundTuple::new();
        let mut prev = GroundTuple::new();
        for binding in cont.rows.iter() {
            ensure_not_cancelled(self.is_cancelled)?;
            tup.clear();
            tup.extend(demand_cols(&cont, stage, binding).map(Option::unwrap));
            if tup < prev {
                cursors.fill(0);
            }
            let present = answers
                .iter()
                .zip(cursors.iter_mut())
                .any(|(blk, cursor)| blk.contains_from(cursor, &tup));
            if !present {
                out.push(stage.values(&cont.clause, binding, &cont.params, &|_| unreachable!()));
                if stage.width() == 0 {
                    break;
                }
            }
            std::mem::swap(&mut prev, &mut tup);
        }
        if !out.is_empty() {
            cont.rows = out;
            self.enqueue(level, Work::Advance(cont));
        }
        Ok(())
    }

    fn resume_negative(
        &mut self,
        level: usize,
        mut cont: Continuation<'a, C>,
    ) -> Result<(), Error> {
        let stage = cont.clause.stage(cont.pos - 1);
        cont.rows = project_bindings(
            &cont.clause,
            stage,
            &cont.params,
            &cont.rows,
            self.is_cancelled,
        )?;
        self.enqueue(level, Work::Advance(cont));
        Ok(())
    }

    fn advance(&mut self, level: usize, mut cont: Continuation<'a, C>) -> Result<(), Error> {
        loop {
            if cont.pos == cont.clause.len() {
                return self.deliver(level, cont);
            }
            let pos = cont.pos;
            cont.pos += 1;
            let stage = cont.clause.stage(pos);
            if !self
                .db
                .snapshot
                .rules_by_head
                .contains_key(cont.clause.relation(stage))
            {
                let rows = join_base(
                    self.db,
                    self.is_cancelled,
                    &cont.clause,
                    stage,
                    &cont.params,
                    &cont.rows,
                )?;
                if rows.is_empty() {
                    return Ok(());
                }
                cont.rows = rows;
                continue;
            }
            return self.dispatch_tabled_stage(level, cont);
        }
    }

    fn deliver(&mut self, level: usize, cont: Continuation<'a, C>) -> Result<(), Error> {
        ensure_not_cancelled(self.is_cancelled)?;
        match cont.dst {
            Dst::Table(table) => self.enqueue(level, Work::Answer(table, cont.rows)),
            Dst::Query => self.rows.extend(cont.rows),
        }
        Ok(())
    }

    fn dispatch_tabled_stage(
        &mut self,
        level: usize,
        cont: Continuation<'a, C>,
    ) -> Result<(), Error> {
        let stage = cont.clause.stage(cont.pos - 1);
        if cont.rows.len() == 1 || !stage.reads_input() {
            let cols = demand_cols(&cont, stage, cont.rows.row(0)).collect();
            return self.dispatch_demand(level, cols, cont);
        }
        if stage.negative {
            return self.dispatch_negative_batch(level, cont);
        }
        let groups = group_demands(&cont, stage, &cont.rows, self.is_cancelled)?;
        for (cols, bindings) in groups {
            self.dispatch_demand(level, cols, cont.with_rows(bindings))?;
        }
        Ok(())
    }

    fn dispatch_negative_batch(
        &mut self,
        level: usize,
        cont: Continuation<'a, C>,
    ) -> Result<(), Error> {
        let stage = cont.clause.stage(cont.pos - 1);
        let rel = cont.clause.relation(stage);
        let first: BoundCols<'a, C> = demand_cols(&cont, stage, cont.rows.row(0)).collect();
        let mut cols = first.clone();
        for binding in cont.rows.iter().skip(1) {
            ensure_not_cancelled(self.is_cancelled)?;
            for (shared, val) in cols.iter_mut().zip(demand_cols(&cont, stage, binding)) {
                if *shared != val {
                    *shared = None;
                }
            }
            if cols.iter().all(Option::is_none) {
                break;
            }
        }
        if cols == first {
            return self.dispatch_demand(level, first, cont);
        }
        let cover = Demand { rel, cols };
        // Probe with the first ground demand first in case recursion already opens `cover`.
        if !self.table_ids.contains_key(&cover) {
            self.ensure_table(Demand { rel, cols: first })?;
        }
        self.enqueue(
            level,
            Work::CheckNegativeBatch(Box::new(NegativeBatch { cover, cont })),
        );
        Ok(())
    }

    fn dispatch_demand(
        &mut self,
        level: usize,
        cols: BoundCols<'a, C>,
        cont: Continuation<'a, C>,
    ) -> Result<(), Error> {
        let stage = cont.clause.stage(cont.pos - 1);
        let rel = cont.clause.relation(stage);
        let id = self.ensure_table(Demand { rel, cols })?;
        if stage.negative {
            self.enqueue(level, Work::CheckNegative(id, cont));
            return Ok(());
        }
        let table = &mut self.tables[id];
        let consumer = Consumer {
            reads_tup: stage.reads_tuple(&cont.params),
            cont,
            stratum: level,
        };
        if let Some(next) = consumer.resume(&table.answers, self.is_cancelled)? {
            push_continuation(&mut self.queues[level], next);
            if !consumer.reads_tup {
                return Ok(());
            }
        }
        table.consumers.push(consumer);
        Ok(())
    }
}
