//! Demand-driven evaluation of rules and queries over one snapshot.
//!
//! The evaluator owns the work queues and the tables they feed: a clause that reaches a tabled
//! relation suspends there, and one demand starts each matching rule once.
mod clause;
mod join;
mod program;
mod tuples;

use self::clause::{Clause, MatchEvidence};
use self::join::{Rows, candidate_row_ids, join_base, project_bindings};
use self::program::Stage;
pub(crate) use self::program::{Program, RuleProgram};
use self::tuples::RowBlock;
use crate::validation::{DeclaredArities, atom, bound_variables, validate_body};
use crate::{Database, Error, ErrorKind, Location, Query};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

/// A borrowed constant, ranked by value. Shared constants compare equal by pointer first, which
/// skips the value comparison in joins.
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
    /// Evaluate this snapshot; cancellation never returns partial rows.
    pub fn query(
        &self,
        query: &Query<C>,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<Vec<C>>, Error> {
        let mut arities = DeclaredArities::extending(&self.snapshot.arities);
        let bound = bound_variables(&query.body);
        validate_body(&query.body, &bound, &mut arities, Location::QueryBody)?;
        for (index, name) in query.select.iter().enumerate() {
            if !bound.contains(name) {
                return Err(Error {
                    location: Location::QuerySelect(index),
                    kind: ErrorKind::UnboundVariable(name.clone()),
                });
            }
        }
        let program = Program::query(query);
        let mut evaluator = Evaluator::new(self, &mut is_cancelled, query.select.len());
        let level = query
            .body
            .iter()
            .map(|literal| evaluator.stratum(&atom(literal).relation.name))
            .max()
            .unwrap_or(0)
            + 1;
        let clause = Rc::new(Clause::query(query, &program));
        let rows = Rows::single_row(clause.initial());
        evaluator.enqueue(
            level,
            Work::Advance(Continuation {
                rows,
                clause,
                parameters: Rc::new(Vec::new()),
                position: 0,
                destination: Destination::Query,
            }),
        );
        evaluator.complete()?;
        evaluator.rows.sort_distinct(evaluator.is_cancelled)?;
        let mut rows = Vec::with_capacity(evaluator.rows.len());
        for row in evaluator.rows.iter() {
            ensure_not_cancelled(evaluator.is_cancelled)?;
            rows.push(row.iter().map(|value| value.0.clone()).collect());
        }
        ensure_not_cancelled(evaluator.is_cancelled)?;
        Ok(rows)
    }
}

/// The columns a demand binds, in column order.
type BoundColumns<'a, C> = SmallVec<[Option<ConstantRef<'a, C>>; 2]>;
/// Memo-table key: a relation and the values its demand binds.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Demand<'a, C: Ord> {
    relation: &'a str,
    columns: BoundColumns<'a, C>,
}
/// Where a finished clause delivers its rows.
#[derive(Clone)]
enum Destination {
    Table(usize),
    Query,
}
/// A clause mid-evaluation: the next stage to run and the rows bound so far.
struct Continuation<'a, C> {
    clause: Rc<Clause<'a, C>>,
    parameters: Rc<Vec<Option<ConstantRef<'a, C>>>>,
    position: usize,
    rows: Rows<'a, C>,
    destination: Destination,
}
impl<'a, C> Continuation<'a, C> {
    fn is_query_output(&self) -> bool {
        matches!(self.destination, Destination::Query) && self.position == self.clause.len()
    }
    fn with_rows(&self, rows: Rows<'a, C>) -> Self {
        Self {
            clause: self.clause.clone(),
            parameters: self.parameters.clone(),
            position: self.position,
            rows,
            destination: self.destination.clone(),
        }
    }
}
/// A clause waiting for one table's answers, and the stratum it resumes in.
struct Consumer<'a, C> {
    continuation: Continuation<'a, C>,
    stratum: usize,
    reads_tuple: bool,
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
        let clause = &self.continuation.clause;
        let parameters = &self.continuation.parameters;
        // The consumer resumes the stage whose table it was waiting on.
        let atom = clause.stage(self.continuation.position - 1);
        let evidence = MatchEvidence::Demand;
        let matches_all = atom.matches_all(parameters, evidence);
        if !self.reads_tuple {
            // Every binding in this demand has the same bound columns and therefore the same
            // matches. The first matching tuple is enough to resume all of them.
            let bindings = &self.continuation.rows;
            for tuple in batches.iter().flat_map(RowBlock::iter) {
                ensure_not_cancelled(is_cancelled)?;
                let value = |column| tuple[column];
                if matches_all
                    || atom.matches(clause, bindings.row(0), parameters, evidence, &value)
                {
                    let rows = project_bindings(clause, atom, parameters, bindings, is_cancelled)?;
                    return Ok(Some(self.continuation.with_rows(rows)));
                }
            }
            return Ok(None);
        }
        let mut rows = Rows::new(atom.width());
        let identity = matches_all && atom.identity(parameters);
        for bindings in self.continuation.rows.iter() {
            for batch in batches {
                rows.append_run(atom.preserves_order && batch.is_strictly_sorted(), |rows| {
                    if identity {
                        rows.append_block(batch);
                        ensure_not_cancelled(is_cancelled)?;
                    } else {
                        for tuple in batch.iter() {
                            ensure_not_cancelled(is_cancelled)?;
                            let value = |column| tuple[column];
                            if matches_all
                                || atom.matches(clause, bindings, parameters, evidence, &value)
                            {
                                rows.push(atom.values(clause, bindings, parameters, &value));
                            }
                        }
                    }
                    Ok(())
                })?;
            }
        }
        Ok((!rows.is_empty()).then(|| self.continuation.with_rows(rows)))
    }
}
/// Answers of one demand and the clauses waiting on them.
struct Table<'a, C> {
    /// Sorted, distinct, pairwise disjoint batches in arrival order.
    answers: Vec<Rows<'a, C>>,
    consumers: Vec<Consumer<'a, C>>,
}
/// One evaluator step.
enum Work<'a, C> {
    /// Continue a clause from its next body atom.
    Advance(Continuation<'a, C>),
    /// New answers for a table.
    Answer(usize, Rows<'a, C>),
    /// Drop rows whose negative atom found an answer.
    CheckNegative(usize, Continuation<'a, C>),
}
fn push_continuation<'a, C: Ord>(queue: &mut VecDeque<Work<'a, C>>, next: Continuation<'a, C>) {
    if next.is_query_output()
        && let Some(Work::Advance(previous)) = queue.back_mut()
        && previous.is_query_output()
        && previous.rows.has_capacity_for(&next.rows)
    {
        // Finished query rows are already projected to select width, so both blocks agree.
        previous.rows.extend(next.rows);
    } else {
        queue.push_back(Work::Advance(next));
    }
}
/// One query's tables, queues and output rows.
struct Evaluator<'a, C: Ord, F> {
    database: &'a Database<C>,
    is_cancelled: &'a mut F,
    rule_clauses: Vec<Option<Rc<Clause<'a, C>>>>,
    table_ids: BTreeMap<Demand<'a, C>, usize>,
    tables: Vec<Table<'a, C>>,
    queues: Vec<VecDeque<Work<'a, C>>>,
    /// Lowest stratum that still has queued work.
    level: usize,
    rows: Rows<'a, C>,
}
/// Splits a tabled stage's input rows into the demands they place on its table. The relation is
/// the same for every row, so only the bound columns key the groups; probing with a borrowed slice
/// keeps rows that join an existing group allocation-free.
fn group_demands<'a, C: Ord>(
    continuation: &Continuation<'a, C>,
    atom: &Stage,
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<BTreeMap<BoundColumns<'a, C>, Rows<'a, C>>, Error> {
    let width = continuation.rows.width();
    let mut columns = BoundColumns::new();
    let mut groups = BTreeMap::<BoundColumns<'a, C>, Rows<'a, C>>::new();
    for row in continuation.rows.iter() {
        ensure_not_cancelled(is_cancelled)?;
        columns.clear();
        for column in 0..atom.arity() {
            columns.push(atom.bound(&continuation.clause, row, &continuation.parameters, column));
        }
        if let Some(bindings) = groups.get_mut(columns.as_slice()) {
            bindings.push(row.iter().cloned());
            continue;
        }
        let mut bindings = Rows::new(width);
        bindings.push(row.iter().cloned());
        groups.insert(columns.clone(), bindings);
    }
    Ok(groups)
}
impl<'a, C: Clone + Ord, F: FnMut() -> bool> Evaluator<'a, C, F> {
    fn new(database: &'a Database<C>, is_cancelled: &'a mut F, output_width: usize) -> Self {
        Self {
            database,
            is_cancelled,
            rule_clauses: (0..database.snapshot.rules.len()).map(|_| None).collect(),
            table_ids: BTreeMap::new(),
            tables: Vec::new(),
            // One queue per stratum plus one for query output, which runs after every stratum.
            queues: (0..=database
                .snapshot
                .strata
                .values()
                .copied()
                .max()
                .unwrap_or(0)
                + 1)
                .map(|_| VecDeque::new())
                .collect(),
            level: 0,
            rows: RowBlock::new(output_width),
        }
    }
    fn enqueue(&mut self, level: usize, work: Work<'a, C>) {
        self.queues[level].push_back(work);
        self.level = self.level.min(level);
    }
    fn stratum(&self, name: &str) -> usize {
        // Only a relation introduced by a query body is absent; nothing derives from it.
        self.database
            .snapshot
            .strata
            .get(name)
            .copied()
            .unwrap_or(0)
    }
    fn ensure_table(&mut self, demand: Demand<'a, C>) -> Result<usize, Error> {
        if let Some(&table) = self.table_ids.get(&demand) {
            return Ok(table);
        }
        let table = self.tables.len();
        self.table_ids.insert(demand.clone(), table);
        let level = self.stratum(demand.relation);
        self.tables.push(Table {
            answers: Vec::new(),
            consumers: Vec::new(),
        });
        self.seed_base_answers(table, &demand, level)?;
        self.start_rule_consumers(table, &demand, level)?;
        Ok(table)
    }
    /// Queues the base relation's rows the demand matches at the table's stratum, so every consumer
    /// of these answers runs in a later round than the answers themselves.
    fn seed_base_answers(
        &mut self,
        table: usize,
        demand: &Demand<'a, C>,
        level: usize,
    ) -> Result<(), Error> {
        let mut tuples = RowBlock::new(demand.columns.len());
        if let Some(relation) = self.database.snapshot.relations.get(demand.relation) {
            let candidates = candidate_row_ids(relation, &demand.columns, self.is_cancelled)?;
            tuples.append_run(true, |tuples| {
                for row in candidates.iter() {
                    ensure_not_cancelled(self.is_cancelled)?;
                    let tuple = relation.row(row);
                    if demand
                        .columns
                        .iter()
                        .zip(tuple)
                        .all(|(bound, value)| bound.is_none_or(|bound| bound == ConstantRef(value)))
                    {
                        tuples.push(tuple.iter().map(ConstantRef));
                    }
                }
                Ok(())
            })?;
        }
        if !tuples.is_empty() {
            self.enqueue(level, Work::Answer(table, tuples));
        }
        Ok(())
    }
    /// Starts every rule whose head matches the demand, with the demand's columns bound as its
    /// parameters and its answers delivering back into the same table.
    fn start_rule_consumers(
        &mut self,
        table: usize,
        demand: &Demand<'a, C>,
        level: usize,
    ) -> Result<(), Error> {
        for &index in self
            .database
            .snapshot
            .rules_by_head
            .get(demand.relation)
            .into_iter()
            .flatten()
        {
            ensure_not_cancelled(self.is_cancelled)?;
            let clause = self.rule_clauses[index]
                .get_or_insert_with(|| Rc::new(Clause::rule(&self.database.snapshot.rules[index])))
                .clone();
            let Some(parameters) = clause.parameters(demand.columns.iter().copied()) else {
                continue;
            };
            let rows = Rows::single_row(clause.initial());
            self.enqueue(
                level,
                Work::Advance(Continuation {
                    clause,
                    parameters: Rc::new(parameters),
                    position: 0,
                    rows,
                    destination: Destination::Table(table),
                }),
            );
        }
        Ok(())
    }
    fn complete(&mut self) -> Result<(), Error> {
        // Lower strata drain before a negative check can observe their tables.
        while let Some((level, work)) = self.next_work() {
            ensure_not_cancelled(self.is_cancelled)?;
            match work {
                Work::Answer(table, tuples) => self.accept_answers(table, tuples),
                Work::CheckNegative(table, continuation) => {
                    self.check_negative(level, table, continuation)
                }
                Work::Advance(continuation) => self.advance(level, continuation),
            }?;
        }
        Ok(())
    }
    /// Takes the next work item, moving to the next stratum only once the current queue drains.
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
    /// Records the answers a table has not seen yet, resumes its consumers with them and folds them
    /// into the table's batch list.
    fn accept_answers(&mut self, table: usize, mut tuples: Rows<'a, C>) -> Result<(), Error> {
        let table = &mut self.tables[table];
        tuples.retain_unseen(&table.answers, self.is_cancelled)?;
        if tuples.is_empty() {
            return Ok(());
        }
        // Drop completed existence consumers while keeping the others in registration order.
        let mut waiting = 0;
        for index in 0..table.consumers.len() {
            let consumer = &table.consumers[index];
            if let Some(next) = consumer.resume(std::slice::from_ref(&tuples), self.is_cancelled)? {
                push_continuation(&mut self.queues[consumer.stratum], next);
                self.level = self.level.min(consumer.stratum);
                if !consumer.reads_tuple {
                    continue;
                }
            }
            if waiting != index {
                table.consumers.swap(waiting, index);
            }
            waiting += 1;
        }
        table.consumers.truncate(waiting);
        // Merge smaller older batches into this one: sizes then at least double towards the tail,
        // keeping later dedup and merge passes logarithmic.
        while table
            .answers
            .last()
            .is_some_and(|old| old.len() <= tuples.len() * 2)
        {
            tuples = table
                .answers
                .pop()
                .unwrap()
                .merge_disjoint(tuples, self.is_cancelled)?;
        }
        table.answers.push(tuples);
        Ok(())
    }
    /// Keeps a clause's rows past a negative stage only when that stage's table stayed empty.
    fn check_negative(
        &mut self,
        level: usize,
        table: usize,
        mut continuation: Continuation<'a, C>,
    ) -> Result<(), Error> {
        if !self.tables[table].answers.is_empty() {
            return Ok(());
        }
        // The stage that demanded this table is the one to keep rows for.
        let atom = continuation.clause.stage(continuation.position - 1);
        continuation.rows = project_bindings(
            &continuation.clause,
            atom,
            &continuation.parameters,
            &continuation.rows,
            self.is_cancelled,
        )?;
        self.enqueue(level, Work::Advance(continuation));
        Ok(())
    }
    /// Runs a clause from its next body atom until it delivers rows or waits on a table.
    fn advance(
        &mut self,
        level: usize,
        mut continuation: Continuation<'a, C>,
    ) -> Result<(), Error> {
        loop {
            if continuation.position == continuation.clause.len() {
                return self.deliver(level, continuation);
            }
            let position = continuation.position;
            continuation.position += 1;
            let atom = continuation.clause.stage(position);
            if !self
                .database
                .snapshot
                .rules_by_head
                .contains_key(continuation.clause.relation(atom))
            {
                let rows = join_base(
                    self.database,
                    self.is_cancelled,
                    &continuation.clause,
                    atom,
                    &continuation.parameters,
                    &continuation.rows,
                )?;
                if rows.is_empty() {
                    return Ok(());
                }
                continuation.rows = rows;
                continue;
            }
            return self.dispatch_tabled_stage(level, &continuation, atom);
        }
    }
    /// Hands a finished clause's rows to its destination.
    fn deliver(&mut self, level: usize, continuation: Continuation<'a, C>) -> Result<(), Error> {
        ensure_not_cancelled(self.is_cancelled)?;
        match continuation.destination {
            Destination::Table(table) => {
                self.enqueue(level, Work::Answer(table, continuation.rows))
            }
            Destination::Query => self.rows.extend(continuation.rows),
        }
        Ok(())
    }
    /// Routes a stage whose relation is tabled: one table per demand, waited on by positive stages
    /// or checked for absence by negative ones.
    fn dispatch_tabled_stage(
        &mut self,
        level: usize,
        continuation: &Continuation<'a, C>,
        atom: &Stage,
    ) -> Result<(), Error> {
        let relation = continuation.clause.relation(atom);
        let reads_tuple = atom.reads_tuple(&continuation.parameters);
        for (columns, bindings) in group_demands(continuation, atom, self.is_cancelled)? {
            let table = self.ensure_table(Demand { relation, columns })?;
            let next = continuation.with_rows(bindings);
            if atom.negative {
                self.enqueue(level, Work::CheckNegative(table, next));
                continue;
            }
            let table = &mut self.tables[table];
            let consumer = Consumer {
                continuation: next,
                stratum: level,
                reads_tuple,
            };
            if let Some(next) = consumer.resume(&table.answers, self.is_cancelled)? {
                push_continuation(&mut self.queues[level], next);
                if !consumer.reads_tuple {
                    continue;
                }
            }
            table.consumers.push(consumer);
        }
        Ok(())
    }
}
