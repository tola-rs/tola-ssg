use crate::validation::{atom, validate_body};
use crate::{Atom, Database, Error, ErrorKind, Literal, Location, Query, Term};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::Hash;

impl<C: Clone + Ord + Hash> Database<C> {
    /// Evaluate this snapshot; cancellation never returns partial rows.
    pub fn query(
        &self,
        query: &Query<C>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<Vec<C>>, Error> {
        let mut arities = self.snapshot.arities.clone();
        let bound = validate_body(&query.body, &mut arities, Location::QueryBody)?;
        for (index, name) in query.select.iter().enumerate() {
            if !bound.contains(name) {
                return Err(Error {
                    location: Location::QuerySelect(index),
                    kind: ErrorKind::UnboundVariable(name.clone()),
                });
            }
        }
        let mut evaluator = Evaluator::new(self, &mut cancelled);
        let level = query
            .body
            .iter()
            .map(|literal| evaluator.stratum(&atom(literal).relation.name))
            .max()
            .unwrap_or(0)
            + 1;
        evaluator
            .queues
            .entry(level)
            .or_default()
            .push_back(Work::Advance(Continuation {
                body: ordered(&query.body),
                position: 0,
                bindings: BTreeMap::new(),
                destination: Destination::Query(query.select.clone()),
            }));
        evaluator.complete()?;
        Ok(evaluator.rows.into_iter().collect())
    }
}
fn ordered<C: Clone>(body: &[Literal<C>]) -> Vec<Literal<C>> {
    body.iter()
        .filter(|literal| matches!(literal, Literal::Positive(_)))
        .chain(
            body.iter()
                .filter(|literal| matches!(literal, Literal::Negative(_))),
        )
        .cloned()
        .collect()
}
type Bindings<C> = BTreeMap<String, C>;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Demand<C> {
    relation: String,
    columns: Vec<Option<C>>,
}
#[derive(Clone)]
enum Destination<C> {
    Table(Demand<C>, Atom<C>),
    Query(Vec<String>),
}
#[derive(Clone)]
struct Continuation<C> {
    body: Vec<Literal<C>>,
    position: usize,
    bindings: Bindings<C>,
    destination: Destination<C>,
}
#[derive(Clone)]
struct Consumer<C> {
    atom: Atom<C>,
    continuation: Continuation<C>,
    stratum: usize,
}
struct Table<C> {
    answers: BTreeSet<Vec<C>>,
    consumers: Vec<Consumer<C>>,
}
enum Work<C> {
    Advance(Continuation<C>),
    Answer(Demand<C>, Vec<C>),
    CheckNegative(Demand<C>, Continuation<C>),
}
struct Evaluator<'a, C, F> {
    database: &'a Database<C>,
    cancelled: &'a mut F,
    tables: BTreeMap<Demand<C>, Table<C>>,
    queues: BTreeMap<usize, VecDeque<Work<C>>>,
    rows: BTreeSet<Vec<C>>,
}
fn demand<C: Clone>(atom: &Atom<C>, bindings: &Bindings<C>) -> Demand<C> {
    Demand {
        relation: atom.relation.name.clone(),
        columns: atom
            .terms
            .iter()
            .map(|term| match term {
                Term::Constant(value) => Some(value.clone()),
                Term::Variable(name) => bindings.get(name).cloned(),
            })
            .collect(),
    }
}
fn unify<C: Clone + Ord>(atom: &Atom<C>, tuple: &[C], bindings: &mut Bindings<C>) -> bool {
    for (term, value) in atom.terms.iter().zip(tuple) {
        match term {
            Term::Constant(expected) if expected != value => return false,
            Term::Variable(name) => match bindings.get(name) {
                Some(expected) if expected != value => return false,
                Some(_) => {}
                None => {
                    bindings.insert(name.clone(), value.clone());
                }
            },
            _ => {}
        }
    }
    true
}
impl<'a, C: Clone + Ord + Hash, F: FnMut() -> bool> Evaluator<'a, C, F> {
    fn new(database: &'a Database<C>, cancelled: &'a mut F) -> Self {
        Self {
            database,
            cancelled,
            tables: BTreeMap::new(),
            queues: BTreeMap::new(),
            rows: BTreeSet::new(),
        }
    }
    fn stratum(&self, name: &str) -> usize {
        self.database
            .snapshot
            .strata
            .get(name)
            .copied()
            .unwrap_or(0)
    }
    // Each consumer sees existing answers once at registration and each later answer once.
    fn ensure_table(&mut self, demand: Demand<C>) -> Result<(), Error> {
        if self.tables.contains_key(&demand) {
            return Ok(());
        }
        let level = self.stratum(&demand.relation);
        self.tables.insert(
            demand.clone(),
            Table {
                answers: BTreeSet::new(),
                consumers: Vec::new(),
            },
        );
        if let Some(index) = self.database.snapshot.relations.get(&demand.relation) {
            let mut candidates = Some(&index.rows);
            for (column, value) in demand.columns.iter().enumerate() {
                if let Some(value) = value {
                    match index.columns[column].get(value) {
                        None => {
                            candidates = None;
                            break;
                        }
                        Some(tuples)
                            if candidates.is_none_or(|current| tuples.len() < current.len()) =>
                        {
                            candidates = Some(tuples)
                        }
                        _ => {}
                    }
                }
            }
            if let Some(tuples) = candidates {
                for &row in tuples {
                    if (self.cancelled)() {
                        return Err(Error {
                            location: Location::Database,
                            kind: ErrorKind::Cancelled,
                        });
                    }
                    let tuple = &self.database.snapshot.facts[row].1;
                    if demand
                        .columns
                        .iter()
                        .zip(tuple)
                        .all(|(bound, value)| bound.as_ref().is_none_or(|bound| bound == value))
                    {
                        self.queues
                            .entry(level)
                            .or_default()
                            .push_back(Work::Answer(demand.clone(), tuple.clone()));
                    }
                }
            }
        }
        for &rule_index in self
            .database
            .snapshot
            .rule_heads
            .get(&demand.relation)
            .into_iter()
            .flatten()
        {
            if (self.cancelled)() {
                return Err(Error {
                    location: Location::Database,
                    kind: ErrorKind::Cancelled,
                });
            }
            let rule = &self.database.snapshot.rules[rule_index];
            let mut bindings = BTreeMap::new();
            let mut matches = true;
            for (term, bound) in rule.head.terms.iter().zip(&demand.columns) {
                if let Some(value) = bound {
                    match term {
                        Term::Constant(expected) if expected != value => {
                            matches = false;
                            break;
                        }
                        Term::Variable(name)
                            if bindings
                                .insert(name.clone(), value.clone())
                                .is_some_and(|previous| previous != *value) =>
                        {
                            matches = false;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            if matches {
                self.queues
                    .entry(level)
                    .or_default()
                    .push_back(Work::Advance(Continuation {
                        body: ordered(&rule.body),
                        position: 0,
                        bindings,
                        destination: Destination::Table(demand.clone(), rule.head.clone()),
                    }));
            }
        }
        Ok(())
    }
    fn complete(&mut self) -> Result<(), Error> {
        // Lower strata drain before a negative check can observe their tables.
        while let Some((level, mut queue)) = self.queues.pop_first() {
            let work = queue.pop_front().unwrap();
            if !queue.is_empty() {
                self.queues.insert(level, queue);
            }
            if (self.cancelled)() {
                return Err(Error {
                    location: Location::Database,
                    kind: ErrorKind::Cancelled,
                });
            }
            match work {
                Work::Answer(demand, tuple) => {
                    let table = self.tables.get_mut(&demand).unwrap();
                    if table.answers.insert(tuple.clone()) {
                        for consumer in table.consumers.clone() {
                            let mut continuation = consumer.continuation;
                            if unify(&consumer.atom, &tuple, &mut continuation.bindings) {
                                self.queues
                                    .entry(consumer.stratum)
                                    .or_default()
                                    .push_back(Work::Advance(continuation));
                            }
                        }
                    }
                }
                Work::CheckNegative(demand, continuation) => {
                    if self.tables[&demand].answers.is_empty() {
                        self.queues
                            .entry(level)
                            .or_default()
                            .push_back(Work::Advance(continuation));
                    }
                }
                Work::Advance(mut continuation) => {
                    if continuation.position == continuation.body.len() {
                        match continuation.destination {
                            Destination::Table(demand, head) => {
                                let tuple = head
                                    .terms
                                    .iter()
                                    .map(|term| match term {
                                        Term::Constant(value) => value.clone(),
                                        Term::Variable(name) => continuation.bindings[name].clone(),
                                    })
                                    .collect();
                                self.queues
                                    .entry(level)
                                    .or_default()
                                    .push_back(Work::Answer(demand, tuple));
                            }
                            Destination::Query(select) => {
                                self.rows.insert(
                                    select
                                        .iter()
                                        .map(|name| continuation.bindings[name].clone())
                                        .collect(),
                                );
                            }
                        }
                        continue;
                    }
                    let literal = continuation.body[continuation.position].clone();
                    continuation.position += 1;
                    let atom = atom(&literal).clone();
                    let demand = demand(&atom, &continuation.bindings);
                    self.ensure_table(demand.clone())?;
                    if matches!(literal, Literal::Negative(_)) {
                        self.queues
                            .entry(level)
                            .or_default()
                            .push_back(Work::CheckNegative(demand, continuation));
                    } else {
                        let table = self.tables.get_mut(&demand).unwrap();
                        for tuple in &table.answers {
                            if (self.cancelled)() {
                                return Err(Error {
                                    location: Location::Database,
                                    kind: ErrorKind::Cancelled,
                                });
                            }
                            let mut next = continuation.clone();
                            if unify(&atom, tuple, &mut next.bindings) {
                                self.queues
                                    .entry(level)
                                    .or_default()
                                    .push_back(Work::Advance(next));
                            }
                        }
                        table.consumers.push(Consumer {
                            atom,
                            continuation,
                            stratum: level,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}
