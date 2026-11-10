#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! In-memory Datalog with demand-driven evaluation and stratified negation.
//!
//! One fixed arity per relation name. Variables are local to one rule or query.
//! Every variable in a rule head, negative atom, or query projection must be
//! bound by a positive atom of the same rule or query; positive atoms are
//! evaluated in their given order, then negative atoms in their given order.
//! A negative atom reads only relations whose answers are complete. Relations
//! without facts or deriving rules are empty. Answers are sorted by `Ord` and
//! duplicate-free.
//!
//! ```
//! use tola_datalog::{Atom, Database, Literal, Query, Relation, Term};
//! let edge = Relation { name: "edge".into(), arity: 2 };
//! let db = Database::new(vec![(edge.clone(), vec![1, 2])], vec![])?;
//! let query = Query {
//!     select: vec!["target".into()],
//!     body: vec![Literal::Positive(Atom {
//!         relation: edge,
//!         terms: vec![Term::Constant(1), Term::Variable("target".into())],
//!     })],
//! };
//! assert_eq!(db.query(&query, || false)?, vec![vec![2]]);
//! # Ok::<(), tola_datalog::Error>(())
//! ```

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

mod evaluation;
mod index;
mod relation;
mod validation;

/// A named relation with a fixed number of columns.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Relation {
    /// Nonempty; all uses of one name must agree on `arity`.
    pub name: String,
    /// Number of columns; zero is a proposition.
    pub arity: usize,
}
/// A variable local to one clause, or a ground value.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Term<C> {
    /// Same name in one clause, same value.
    Variable(String),
    /// Matched exactly.
    Constant(C),
}
/// A relation applied to one term per column.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Atom<C> {
    /// The relation this atom reads.
    pub relation: Relation,
    /// One term per column, in the relation's column order.
    pub terms: Vec<Term<C>>,
}
impl<C> Atom<C> {
    /// Hash the atom using the caller's encoding for constants.
    pub fn hash_with<H: Hasher>(&self, hasher: &mut H, hash_constant: impl Fn(&C, &mut H)) {
        self.relation.hash(hasher);
        self.terms.len().hash(hasher);
        for term in &self.terms {
            std::mem::discriminant(term).hash(hasher);
            match term {
                Term::Variable(name) => name.hash(hasher),
                Term::Constant(value) => hash_constant(value, hasher),
            }
        }
    }
}
impl<C: Hash> Hash for Atom<C> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.hash_with(hasher, C::hash);
    }
}
/// A positive match or the absence of a fully bound match.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Literal<C> {
    /// Holds when at least one tuple matches.
    Positive(Atom<C>),
    /// Holds when the ground tuple is absent.
    Negative(Atom<C>),
}
impl<C> Literal<C> {
    fn hash_with<H: Hasher>(&self, hasher: &mut H, hash_constant: impl Fn(&C, &mut H)) {
        std::mem::discriminant(self).hash(hasher);
        match self {
            Self::Positive(atom) | Self::Negative(atom) => atom.hash_with(hasher, hash_constant),
        }
    }
}
impl<C: Hash> Hash for Literal<C> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.hash_with(hasher, C::hash);
    }
}
/// A rule deriving its head from all literals in its body.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Rule<C> {
    /// The atom this rule derives, over the variables its body binds.
    pub head: Atom<C>,
    /// Body literals. Positive literals are evaluated before negative literals.
    pub body: Vec<Literal<C>>,
}
/// A conjunctive query with an ordered projection.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Query<C> {
    /// Output variables in column order; repetition is allowed.
    pub select: Vec<String>,
    /// Body literals. Positive literals are evaluated before negative literals.
    pub body: Vec<Literal<C>>,
}

/// Zero-based position in the caller's input, counted as the caller supplied it, before any
/// canonical ordering; `Database` is the exception, recording that no single position applies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Location {
    /// Row of `Database::new` or `from_rows`, counted across all relations.
    Fact(usize),
    /// Clause in the `rules` argument.
    RuleHead(usize),
    /// A rule body atom, counted as the rule writes it.
    RuleBody {
        /// Clause in the `rules` argument.
        rule: usize,
        /// Atom in that clause's body.
        atom: usize,
    },
    /// Atom of the query body.
    QueryBody(usize),
    /// Variable of the query projection.
    QuerySelect(usize),
    /// No single input position, e.g. cancellation.
    Database,
}
/// Validation or evaluation failure, independent of its source location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// A relation use supplied a different number of terms than its relation declares.
    Arity {
        /// The arity the relation declares.
        expected: usize,
        /// The number of terms this use supplied.
        actual: usize,
    },
    /// One name was used with two arities; reported at the later use.
    RelationArity {
        /// The relation name.
        name: String,
        /// The arity the earlier use declared.
        expected: usize,
        /// The arity this use supplied.
        actual: usize,
    },
    /// No positive atom in the clause binds this variable.
    UnboundVariable(String),
    /// Relation names must not be empty.
    EmptyRelation,
    /// A relation depends negatively on itself, directly or indirectly.
    NegativeCycle,
    /// The cancellation callback returned true before the result was complete.
    Cancelled,
}

/// A failure and where in the caller's input it occurred.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{}", kind_message(.kind))]
pub struct Error {
    /// Where in the caller's input the failure was noticed.
    pub location: Location,
    /// What failed.
    pub kind: ErrorKind,
}

/// The sentence one failure kind renders; the location stays out of the message.
fn kind_message(kind: &ErrorKind) -> String {
    match kind {
        ErrorKind::Arity { expected, actual } => {
            format!("expected {expected} terms, found {actual}")
        }
        ErrorKind::RelationArity {
            name,
            expected,
            actual,
        } => format!("relation `{name}` has arity {actual}, expected {expected}"),
        ErrorKind::UnboundVariable(name) => format!("variable `{name}` has no positive binding"),
        ErrorKind::EmptyRelation => "relation name must not be empty".to_owned(),
        ErrorKind::NegativeCycle => "negation creates a recursive dependency".to_owned(),
        ErrorKind::Cancelled => "query cancelled".to_owned(),
    }
}

/// An immutable snapshot of facts and rules; `Clone` shares it instead of copying.
#[derive(Debug)]
pub struct Database<C> {
    pub(crate) snapshot: Arc<Snapshot<C>>,
}
impl<C> Clone for Database<C> {
    // Hand-written so cloning does not require `C: Clone`.
    fn clone(&self) -> Self {
        Self {
            snapshot: self.snapshot.clone(),
        }
    }
}

/// The validated data every query reads.
#[derive(Debug)]
pub(crate) struct Snapshot<C> {
    /// Canonical rule order; `rules_by_head` indexes into it.
    pub(crate) rules: Vec<evaluation::RuleProgram<C>>,
    /// One arity per relation name mentioned by a fact or clause.
    pub(crate) arities: BTreeMap<String, usize>,
    /// Stratum of every name in `arities`.
    pub(crate) strata: BTreeMap<String, usize>,
    /// Frozen rows of each relation that has at least one fact.
    pub(crate) relations: BTreeMap<String, relation::RelationRows<C>>,
    /// Rule indexes deriving each relation name.
    pub(crate) rules_by_head: BTreeMap<String, Vec<usize>>,
}
// Equality and hashing cover exactly the inputs a snapshot was built from; arities, strata,
// programs and row indexes are derived from them and ignored.
impl<C: PartialEq> PartialEq for Database<C> {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.relations == other.snapshot.relations
            && self
                .snapshot
                .rules
                .iter()
                .map(|compiled| &compiled.rule)
                .eq(other.snapshot.rules.iter().map(|compiled| &compiled.rule))
    }
}
impl<C: Eq> Eq for Database<C> {}
impl<C> Database<C> {
    /// Hash the stored rows and rules using the caller's encoding for constants.
    ///
    /// This can distinguish representations that compare equal during evaluation.
    pub fn hash_with<H: Hasher>(&self, hasher: &mut H, hash_constant: impl Fn(&C, &mut H)) {
        self.snapshot.relations.len().hash(hasher);
        for (name, rows) in &self.snapshot.relations {
            name.hash(hasher);
            rows.hash_with(hasher, &hash_constant);
        }
        self.snapshot.rules.len().hash(hasher);
        for compiled in &self.snapshot.rules {
            compiled.rule.head.hash_with(hasher, &hash_constant);
            compiled.rule.body.len().hash(hasher);
            for literal in &compiled.rule.body {
                literal.hash_with(hasher, &hash_constant);
            }
        }
    }
}
impl<C: Hash> Hash for Database<C> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.hash_with(hasher, C::hash);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::{atom, variables};
    use std::collections::BTreeSet;

    fn relation(name: &str, arity: usize) -> Relation {
        Relation {
            name: name.into(),
            arity,
        }
    }
    fn var<C>(name: &str) -> Term<C> {
        Term::Variable(name.into())
    }
    fn constant(value: i32) -> Term<i32> {
        Term::Constant(value)
    }
    fn atom_of<C>(name: &str, terms: Vec<Term<C>>) -> Atom<C> {
        Atom {
            relation: relation(name, terms.len()),
            terms,
        }
    }
    fn positive<C>(name: &str, terms: Vec<Term<C>>) -> Literal<C> {
        Literal::Positive(atom_of(name, terms))
    }
    fn negative<C>(name: &str, terms: Vec<Term<C>>) -> Literal<C> {
        Literal::Negative(atom_of(name, terms))
    }
    fn edge(from: i32, to: i32) -> (Relation, Vec<i32>) {
        (relation("edge", 2), vec![from, to])
    }
    fn paths() -> Vec<Rule<i32>> {
        vec![
            Rule {
                head: atom_of("path", vec![var("x"), var("y")]),
                body: vec![positive("edge", vec![var("x"), var("y")])],
            },
            Rule {
                head: atom_of("path", vec![var("x"), var("z")]),
                body: vec![
                    positive("path", vec![var("x"), var("y")]),
                    positive("edge", vec![var("y"), var("z")]),
                ],
            },
        ]
    }
    fn query(select: &[&str], body: Vec<Literal<i32>>) -> Query<i32> {
        Query {
            select: select.iter().map(|name| (*name).into()).collect(),
            body,
        }
    }
    fn one_atom_query<C>(select: &[&str], relation: &Relation, terms: Vec<Term<C>>) -> Query<C> {
        Query {
            select: select.iter().map(|name| (*name).into()).collect(),
            body: vec![Literal::Positive(Atom {
                relation: relation.clone(),
                terms,
            })],
        }
    }
    fn answer_rows(database: &Database<i32>, query: &Query<i32>) -> Vec<Vec<i32>> {
        database.query(query, || false).unwrap()
    }
    fn new_database<C: Ord>(facts: Vec<(Relation, Vec<C>)>, rules: Vec<Rule<C>>) -> Database<C> {
        Database::new(facts, rules).unwrap()
    }

    // Enumerate ground substitutions independently of the tabled evaluator.
    fn reference_answers(
        facts: &[(Relation, Vec<i32>)],
        rules: &[Rule<i32>],
        domain: &[i32],
    ) -> BTreeSet<(String, Vec<i32>)> {
        let mut tuples: BTreeSet<_> = facts
            .iter()
            .map(|(relation, tuple)| (relation.name.clone(), tuple.clone()))
            .collect();
        let mut strata = BTreeMap::<String, usize>::new();
        for rule in rules {
            strata.entry(rule.head.relation.name.clone()).or_default();
            for literal in &rule.body {
                strata
                    .entry(atom(literal).relation.name.clone())
                    .or_default();
            }
        }
        loop {
            let mut changed = false;
            for rule in rules {
                for literal in &rule.body {
                    let needed = strata[&atom(literal).relation.name]
                        + usize::from(matches!(literal, Literal::Negative(_)));
                    let level = strata.get_mut(&rule.head.relation.name).unwrap();
                    if *level < needed {
                        *level = needed;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for level in 0..=strata.values().copied().max().unwrap_or(0) {
            loop {
                let mut added = Vec::new();
                for rule in rules
                    .iter()
                    .filter(|rule| strata[&rule.head.relation.name] == level)
                {
                    let names: BTreeSet<_> = variables(&rule.head)
                        .chain(
                            rule.body
                                .iter()
                                .flat_map(|literal| variables(atom(literal))),
                        )
                        .cloned()
                        .collect();
                    let names: Vec<_> = names.into_iter().collect();
                    let count = domain.len().pow(names.len() as u32);
                    for mut assignment in 0..count {
                        let mut values = BTreeMap::new();
                        for name in &names {
                            values.insert(name.clone(), domain[assignment % domain.len()]);
                            assignment /= domain.len();
                        }
                        let ground = |atom: &Atom<i32>| {
                            atom.terms
                                .iter()
                                .map(|term| match term {
                                    Term::Constant(value) => *value,
                                    Term::Variable(name) => values[name],
                                })
                                .collect::<Vec<_>>()
                        };
                        if rule.body.iter().all(|literal| {
                            let present = tuples.contains(&(
                                atom(literal).relation.name.clone(),
                                ground(atom(literal)),
                            ));
                            present == matches!(literal, Literal::Positive(_))
                        }) {
                            added.push((rule.head.relation.name.clone(), ground(&rule.head)));
                        }
                    }
                }
                let before = tuples.len();
                tuples.extend(added);
                if before == tuples.len() {
                    break;
                }
            }
        }
        tuples
    }

    #[test]
    fn recursive_answers_match_reference_for_every_edge_subset() {
        let mut rules = paths();
        rules.push(Rule {
            head: atom_of("indirect", vec![var("x"), var("y")]),
            body: vec![
                negative("edge", vec![var("x"), var("y")]),
                positive("path", vec![var("x"), var("y")]),
            ],
        });
        for mask in 0..512 {
            let facts: Vec<_> = (0..9)
                .filter(|bit| mask & (1 << bit) != 0)
                .map(|bit| edge(bit / 3, bit % 3))
                .collect();
            let expected = reference_answers(&facts, &rules, &[0, 1, 2]);
            let database = new_database(facts, rules.clone());
            for source in 0..3 {
                for name in ["path", "indirect"] {
                    let actual = answer_rows(
                        &database,
                        &query(
                            &["y"],
                            vec![positive(name, vec![constant(source), var("y")])],
                        ),
                    );
                    let expected_rows: Vec<_> = expected
                        .iter()
                        .filter(|(relation, tuple)| relation == name && tuple[0] == source)
                        .map(|(_, tuple)| vec![tuple[1]])
                        .collect();
                    assert_eq!(
                        actual, expected_rows,
                        "mask {mask}, {name}, source {source}"
                    );
                }
            }
        }
    }

    #[test]
    fn projection_is_sorted_and_distinct() {
        let database = new_database(
            vec![
                edge(2, 0),
                edge(1, 2),
                edge(1, 1),
                edge(2, 2),
                edge(1, 1),
                edge(0, 2),
            ],
            vec![],
        );
        let cases = [
            (
                query(
                    &["x", "x"],
                    vec![positive("edge", vec![var("x"), var("x")])],
                ),
                vec![vec![1, 1], vec![2, 2]],
            ),
            (
                query(
                    &["y", "x"],
                    vec![positive("edge", vec![var("x"), var("y")])],
                ),
                vec![vec![0, 2], vec![1, 1], vec![2, 0], vec![2, 1], vec![2, 2]],
            ),
            (
                query(&["y"], vec![positive("edge", vec![var("x"), var("y")])]),
                vec![vec![0], vec![1], vec![2]],
            ),
            (
                query(&[], vec![positive("edge", vec![var("x"), var("y")])]),
                vec![vec![]],
            ),
        ];
        for (query, expected) in cases {
            assert_eq!(answer_rows(&database, &query), expected);
        }
    }

    #[test]
    fn recursive_self_join_matches_reference_on_cyclic_edges() {
        let mut rules = paths();
        rules[1].body[1] = positive("path", vec![var("y"), var("z")]);
        let facts = vec![edge(0, 1), edge(1, 2), edge(2, 0), edge(2, 3)];
        let expected = reference_answers(&facts, &rules, &[0, 1, 2, 3]);
        let database = new_database(facts, rules);
        let actual = answer_rows(
            &database,
            &query(
                &["x", "z"],
                vec![positive("path", vec![var("x"), var("z")])],
            ),
        );
        let expected: Vec<_> = expected
            .into_iter()
            .filter(|(name, _)| name == "path")
            .map(|(_, tuple)| tuple)
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn mutual_recursion_matches_reference_under_renaming_and_reordering() {
        let facts: Vec<_> = [[0, 1, 2], [0, 2, 1], [1, 2, 0], [2, 1, 0]]
            .into_iter()
            .map(|tuple| (relation("triple", 3), tuple.to_vec()))
            .collect();
        let rules = vec![
            Rule {
                head: atom_of("p", vec![var("x"), var("y"), var("z")]),
                body: vec![positive("triple", vec![var("x"), var("y"), var("z")])],
            },
            Rule {
                head: atom_of("q", vec![var("x"), var("z"), var("y")]),
                body: vec![positive("p", vec![var("x"), var("y"), var("z")])],
            },
            Rule {
                head: atom_of("p", vec![var("x"), var("y"), var("w")]),
                body: vec![
                    positive("q", vec![var("x"), var("z"), var("y")]),
                    positive("triple", vec![var("y"), var("z"), var("w")]),
                ],
            },
        ];
        let all = reference_answers(&facts, &rules, &[0, 1, 2]);
        let tuples: Vec<_> = all
            .iter()
            .filter(|(name, _)| name == "p")
            .map(|(_, tuple)| tuple)
            .collect();
        let expected: Vec<_> = tuples
            .iter()
            .flat_map(|left| {
                tuples
                    .iter()
                    .filter(move |right| left[0] == right[0] && left[2] == right[1])
                    .map(move |right| vec![left[0], left[1], right[2]])
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let body = vec![
            positive("p", vec![var("x"), var("y"), var("z")]),
            positive("p", vec![var("x"), var("z"), var("w")]),
        ];
        let database = new_database(facts.clone(), rules.clone());
        assert_eq!(
            answer_rows(&database, &query(&["x", "y", "w"], body.clone())),
            expected
        );

        fn rename_atom(atom: &mut Atom<i32>) {
            for term in &mut atom.terms {
                if let Term::Variable(name) = term {
                    *name = format!("renamed_{name}");
                }
            }
        }
        fn rename_literal(literal: &mut Literal<i32>) {
            match literal {
                Literal::Positive(atom) | Literal::Negative(atom) => rename_atom(atom),
            }
        }
        let mut renamed = rules;
        for rule in &mut renamed {
            rename_atom(&mut rule.head);
            for literal in &mut rule.body {
                rename_literal(literal);
            }
            rule.body.reverse();
        }
        renamed.reverse();
        let mut renamed_body = body;
        for literal in &mut renamed_body {
            rename_literal(literal);
        }
        renamed_body.reverse();
        let renamed_database = new_database(facts, renamed);
        assert_eq!(
            answer_rows(
                &renamed_database,
                &query(&["renamed_x", "renamed_y", "renamed_w"], renamed_body)
            ),
            expected
        );
    }

    #[test]
    fn nullary_relations_project_empty_rows() {
        let rules = vec![
            Rule {
                head: atom_of("seed", vec![constant(7)]),
                body: vec![],
            },
            Rule {
                head: atom_of("enabled", vec![]),
                body: vec![],
            },
            Rule {
                head: atom_of("ready", vec![]),
                body: vec![
                    positive("seed", vec![constant(7)]),
                    positive("enabled", vec![]),
                    negative("missing", vec![]),
                ],
            },
        ];
        let database = new_database(vec![], rules);
        assert_eq!(
            answer_rows(&database, &query(&[], vec![positive("ready", vec![])])),
            vec![vec![]]
        );
        assert!(answer_rows(&database, &query(&[], vec![positive("missing", vec![])])).is_empty());
        assert_eq!(answer_rows(&database, &query(&[], vec![])), vec![vec![]]);
    }

    #[test]
    fn deep_negation_uses_bounded_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let rules: Vec<_> = (1..=1401)
                    .map(|level| Rule {
                        head: atom_of(&format!("level{level}"), vec![]),
                        body: vec![negative(&format!("level{}", level - 1), vec![])],
                    })
                    .collect();
                let database = new_database(vec![(relation("level0", 0), vec![])], rules);
                assert_eq!(
                    answer_rows(&database, &query(&[], vec![positive("level1400", vec![])])),
                    vec![vec![]]
                );
                assert!(
                    answer_rows(&database, &query(&[], vec![positive("level1401", vec![])]))
                        .is_empty()
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn snapshot_identity_ignores_order_and_duplicates() {
        let rules = paths();
        let first = new_database(vec![edge(1, 2), edge(2, 3)], rules.clone());
        let mut reordered = rules;
        reordered.reverse();
        reordered.push(reordered[0].clone());
        let second = new_database(vec![edge(2, 3), edge(1, 2), edge(1, 2)], reordered);
        assert_eq!(first, second);
        use std::hash::DefaultHasher;
        let mut first_hash = DefaultHasher::new();
        let mut second_hash = DefaultHasher::new();
        first.hash(&mut first_hash);
        second.hash(&mut second_hash);
        assert_eq!(first_hash.finish(), second_hash.finish());
    }

    #[test]
    fn from_rows_rejects_wrong_arity() {
        let relation = relation("value", usize::MAX);
        let error = Database::<i32>::from_rows([(&relation, [])], vec![]).unwrap_err();
        assert_eq!(
            error,
            Error {
                location: Location::Fact(0),
                kind: ErrorKind::Arity {
                    expected: usize::MAX,
                    actual: 0
                },
            }
        );
    }

    #[test]
    fn from_rows_dedup_keeps_first_equal_value() {
        let relation = relation("value", 1);
        let first: Arc<str> = Arc::from("same");
        let duplicate: Arc<str> = Arc::from("same");
        let rows = [Arc::from("later"), first.clone(), duplicate];
        let database =
            Database::from_rows(rows.into_iter().map(|value| (&relation, [value])), vec![])
                .unwrap();
        let query = one_atom_query(&["x"], &relation, vec![var("x")]);
        let rows = database.query(&query, || false).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(Arc::ptr_eq(&rows[1][0], &first));
    }

    #[test]
    fn answers_depend_only_on_their_snapshot() {
        let rules = paths();
        let old = new_database(vec![edge(1, 2), edge(2, 3)], rules.clone());
        let new = new_database(vec![edge(1, 2)], rules);
        let q = query(&["y"], vec![positive("path", vec![constant(1), var("y")])]);
        assert_eq!(answer_rows(&old, &q), vec![vec![2], vec![3]]);
        assert_eq!(answer_rows(&new, &q), vec![vec![2]]);
    }

    #[test]
    fn validation_errors_locate_the_invalid_input() {
        let error = Database::new(
            vec![edge(1, 2)],
            vec![Rule {
                head: atom_of("edge", vec![var("x")]),
                body: vec![positive("edge", vec![var("x"), constant(2)])],
            }],
        )
        .unwrap_err();
        assert_eq!(
            error,
            Error {
                location: Location::RuleHead(0),
                kind: ErrorKind::RelationArity {
                    name: "edge".into(),
                    expected: 2,
                    actual: 1
                }
            }
        );
        let error = Database::<i32>::new(
            vec![],
            vec![Rule {
                head: atom_of("p", vec![var("x")]),
                body: vec![negative("q", vec![var("x")])],
            }],
        )
        .unwrap_err();
        assert_eq!(
            error,
            Error {
                location: Location::RuleBody { rule: 0, atom: 0 },
                kind: ErrorKind::UnboundVariable("x".into())
            }
        );
        let error = Database::<i32>::new(vec![(relation("", 0), vec![])], vec![]).unwrap_err();
        assert_eq!(
            error,
            Error {
                location: Location::Fact(0),
                kind: ErrorKind::EmptyRelation
            }
        );
        let database = new_database(vec![], vec![]);
        assert_eq!(
            database
                .query(&query(&["x"], vec![]), || false)
                .unwrap_err()
                .location,
            Location::QuerySelect(0)
        );
    }

    #[test]
    fn validation_reports_the_first_invalid_body_atom() {
        let malformed = Literal::Positive(Atom {
            relation: relation("wrong", 1),
            terms: vec![],
        });
        let database = new_database(vec![], vec![]);
        let query = query(&[], vec![negative("q", vec![var("x")]), malformed]);
        let error = database.query(&query, || false).unwrap_err();
        assert_eq!(error.location, Location::QueryBody(0));
        assert_eq!(error.kind, ErrorKind::UnboundVariable("x".into()));
    }

    #[test]
    fn existential_join_waits_for_a_matching_tabled_answer() {
        let mut facts = vec![
            (relation("pair", 2), vec![1, 2]),
            (relation("input", 1), vec![10]),
            (relation("input", 1), vec![20]),
            (relation("exclude", 1), vec![20]),
        ];
        let rules = vec![Rule {
            head: atom_of("pair", vec![var("y"), var("y")]),
            body: vec![positive("diagonal", vec![var("y")])],
        }];
        let query = query(
            &["x"],
            vec![
                positive("input", vec![var("x")]),
                positive("pair", vec![var("y"), var("y")]),
                negative("exclude", vec![var("x")]),
            ],
        );
        let before = new_database(facts.clone(), rules.clone());
        assert!(answer_rows(&before, &query).is_empty());
        facts.push((relation("diagonal", 1), vec![3]));
        let after = new_database(facts, rules);
        assert_eq!(answer_rows(&after, &query), vec![vec![10]]);
    }

    #[test]
    fn from_rows_pulls_values_before_dropping_each_source() {
        use std::borrow::Borrow;
        use std::cell::Cell;
        use std::rc::Rc;

        struct Source {
            relation: Relation,
            dropped: Rc<Cell<bool>>,
        }
        impl Borrow<Relation> for Source {
            fn borrow(&self) -> &Relation {
                &self.relation
            }
        }
        impl Drop for Source {
            fn drop(&mut self) {
                self.dropped.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let observed = dropped.clone();
        let relation = relation("value", 1);
        let source = Source {
            relation: relation.clone(),
            dropped: dropped.clone(),
        };
        let values = std::iter::once_with(move || if observed.get() { 2 } else { 1 });
        let database = Database::from_rows([(source, values)], vec![]).unwrap();
        assert!(dropped.get());
        let query = one_atom_query(&["x"], &relation, vec![var("x")]);
        assert_eq!(answer_rows(&database, &query), vec![vec![1]]);
    }

    #[test]
    fn relation_declarations_validate_in_input_order() {
        let cases = [
            (
                vec![(relation("p", 2), vec![1]), (relation("", 0), vec![])],
                Location::Fact(0),
                ErrorKind::Arity {
                    expected: 2,
                    actual: 1,
                },
            ),
            (
                vec![(relation("p", 2), vec![1, 2]), (relation("p", 1), vec![1])],
                Location::Fact(1),
                ErrorKind::RelationArity {
                    name: "p".into(),
                    expected: 2,
                    actual: 1,
                },
            ),
            (
                vec![(relation("p", 2), vec![1, 2]), (relation("p", 1), vec![])],
                Location::Fact(1),
                ErrorKind::Arity {
                    expected: 1,
                    actual: 0,
                },
            ),
        ];
        for (rows, location, kind) in cases {
            assert_eq!(
                Database::new(rows, vec![]).unwrap_err(),
                Error { location, kind }
            );
        }
    }

    #[test]
    fn negative_cycle_names_closing_atom() {
        for self_cycle in [false, true] {
            for negative_first in [false, true] {
                for reverse in [false, true] {
                    let mut rules = vec![
                        Rule {
                            head: atom_of("downstream", vec![var("x")]),
                            body: vec![positive("p", vec![var("x")])],
                        },
                        Rule {
                            head: atom_of("outside", vec![var("x")]),
                            body: vec![
                                positive("domain", vec![var("x")]),
                                negative("p", vec![var("x")]),
                            ],
                        },
                        Rule {
                            head: atom_of("p", vec![var("x")]),
                            body: vec![
                                positive("domain", vec![var("x")]),
                                negative(if self_cycle { "p" } else { "q" }, vec![var("x")]),
                            ],
                        },
                    ];
                    if !self_cycle {
                        rules.push(Rule {
                            head: atom_of("q", vec![var("x")]),
                            body: vec![positive("p", vec![var("x")])],
                        });
                    }
                    if negative_first {
                        rules.swap(0, 1);
                    }
                    if reverse {
                        rules.reverse();
                    }
                    let rule = rules
                        .iter()
                        .position(|rule| rule.head.relation.name == "p")
                        .unwrap();
                    assert_eq!(
                        Database::<i32>::new(vec![], rules).unwrap_err(),
                        Error {
                            location: Location::RuleBody { rule, atom: 1 },
                            kind: ErrorKind::NegativeCycle,
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn negation_waits_for_recursive_answers() {
        let mut tuples = vec![edge(0, 1), edge(1, 2)];
        tuples.extend((0..3).map(|id| (relation("node", 1), vec![id])));
        let mut rules = paths();
        rules.extend([
            Rule {
                head: atom_of("missing", vec![var("x")]),
                body: vec![
                    positive("node", vec![var("x")]),
                    negative("path", vec![var("x"), constant(2)]),
                ],
            },
            Rule {
                head: atom_of("present", vec![var("x")]),
                body: vec![
                    positive("node", vec![var("x")]),
                    negative("missing", vec![var("x")]),
                ],
            },
        ]);
        let database = new_database(tuples, rules);
        assert_eq!(
            answer_rows(
                &database,
                &query(&["x"], vec![positive("present", vec![var("x")])]),
            ),
            vec![vec![0], vec![1]]
        );
    }

    #[test]
    fn repeated_head_variable_matches_only_equal_columns() {
        let database = new_database(
            vec![
                (relation("node", 1), vec![1]),
                (relation("node", 1), vec![2]),
            ],
            vec![Rule {
                head: atom_of("same", vec![var("x"), var("x")]),
                body: vec![positive("node", vec![var("x")])],
            }],
        );
        assert_eq!(
            answer_rows(
                &database,
                &query(&["y"], vec![positive("same", vec![constant(1), var("y")])]),
            ),
            vec![vec![1]]
        );
        assert!(
            answer_rows(
                &database,
                &query(&[], vec![positive("same", vec![constant(1), constant(2)])]),
            )
            .is_empty()
        );
    }

    #[test]
    fn projection_dedup_retains_first_value_source() {
        let pair = relation("pair", 2);
        let values: Vec<Arc<str>> = (0..64)
            .map(|index| Arc::from((index % 3).to_string()))
            .collect();
        let tuples = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                (
                    pair.clone(),
                    vec![Arc::from(format!("{index:02}")), value.clone()],
                )
            })
            .collect();
        let database = new_database(tuples, vec![]);
        let query = one_atom_query(&["value"], &pair, vec![var("key"), var("value")]);
        let rows = database.query(&query, || false).unwrap();
        assert_eq!(rows.len(), 3);
        for (row, first) in rows.iter().zip(&values) {
            assert!(Arc::ptr_eq(&row[0], first));
        }
    }

    #[test]
    fn bound_recursion_has_no_iteration_limit() {
        let facts: Vec<_> = (0..10_050)
            .map(|i| edge(i, i + 1))
            .chain((20_000..30_000).map(|i| edge(i, i + 1)))
            .collect();
        let database = new_database(facts, paths());
        let rows = answer_rows(
            &database,
            &query(&["y"], vec![positive("path", vec![constant(0), var("y")])]),
        );
        assert_eq!(rows.len(), 10_050);
        assert_eq!(rows.last(), Some(&vec![10_050]));
    }

    #[test]
    fn repeated_bound_queries_remain_independent() {
        let facts = (0..500)
            .flat_map(|component| {
                let source = component * 3;
                [edge(source, source + 1), edge(source + 1, source + 2)]
            })
            .collect();
        let database = new_database(facts, paths());
        for component in 0..500 {
            let source = component * 3;
            let q = query(
                &["y"],
                vec![positive("path", vec![constant(source), var("y")])],
            );
            assert_eq!(
                answer_rows(&database, &q),
                vec![vec![source + 1], vec![source + 2]]
            );
        }
    }

    #[test]
    fn cancellation_returns_error() {
        let database = new_database(vec![edge(1, 2), edge(2, 3)], paths());
        let mut calls = 0;
        let error = database
            .query(
                &query(&["y"], vec![positive("path", vec![constant(1), var("y")])]),
                || {
                    calls += 1;
                    calls > 8
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
    }

    #[test]
    fn output_materialization_observes_cancellation() {
        use std::cell::Cell;

        #[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
        struct Key<'a> {
            ordinal: u8,
            cancelled: &'a Cell<bool>,
        }
        impl Clone for Key<'_> {
            fn clone(&self) -> Self {
                self.cancelled.set(true);
                Self {
                    ordinal: self.ordinal,
                    cancelled: self.cancelled,
                }
            }
        }

        for count in [1, 2] {
            // The shared cell cannot change the keys' relative ordering.
            let cancelled = Cell::new(false);
            let relation = relation("key", 1);
            let facts = (1..=count)
                .map(|ordinal| {
                    (
                        relation.clone(),
                        vec![Key {
                            ordinal,
                            cancelled: &cancelled,
                        }],
                    )
                })
                .collect();
            let database = new_database(facts, vec![]);
            let query = one_atom_query(&["key"], &relation, vec![var("key")]);
            let error = database.query(&query, || cancelled.get()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Cancelled);
        }
    }
}
