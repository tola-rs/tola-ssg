#![forbid(unsafe_code)]
//! In-memory Datalog with demand-driven evaluation and stratified negation.
//!
//! Relations have one fixed arity per name. Variables are local to a rule or
//! query. Every variable in a rule head, negative atom, or query projection must
//! appear in a positive atom in the same rule or query. Positive atoms are
//! evaluated in their given order; negative atoms follow them after their
//! dependencies reach a fixed point. Answers are sorted, duplicate-free tuples.
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
mod validation;

/// A named relation with a fixed number of columns.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Relation {
    /// Nonempty relation name, compared exactly.
    pub name: String,
    /// Number of columns, including zero for a proposition.
    pub arity: usize,
}
/// A variable local to one clause or a ground value.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Term<C> {
    /// Variables with the same name in one clause refer to the same value.
    Variable(String),
    /// A value matched exactly.
    Constant(C),
}
/// A relation applied to one term per column.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Atom<C> {
    /// Relation to match.
    pub relation: Relation,
    /// Terms in column order.
    pub terms: Vec<Term<C>>,
}
/// A positive match or the absence of a fully bound match.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Literal<C> {
    /// Match tuples in this relation.
    Positive(Atom<C>),
    /// Match only if this ground tuple is absent.
    Negative(Atom<C>),
}
/// A rule deriving its head from all literals in its body.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Rule<C> {
    /// Derived relation and columns.
    pub head: Atom<C>,
    /// Conditions for deriving the head.
    pub body: Vec<Literal<C>>,
}
/// A conjunctive query with an ordered projection.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Query<C> {
    /// Variable names in output-column order; repetition is allowed.
    pub select: Vec<String>,
    /// Conditions matched by every returned row.
    pub body: Vec<Literal<C>>,
}

/// Zero-based position of an invalid clause or tuple in the caller's input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Location {
    /// Index of a ground tuple in the database input.
    Fact(usize),
    /// Index of a rule whose head is invalid.
    RuleHead(usize),
    /// Indexes of a rule and its invalid body atom.
    RuleBody { rule: usize, atom: usize },
    /// Index of an invalid query atom.
    QueryBody(usize),
    /// Index of an invalid projected variable.
    QuerySelect(usize),
    /// An error affecting the whole database or query.
    Database,
}
/// Validation or evaluation failure, independent of its source location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// The tuple or atom has the wrong number of columns.
    Arity {
        /// Declared number of columns.
        expected: usize,
        /// Supplied number of columns.
        actual: usize,
    },
    /// One name was used with two different arities.
    RelationArity {
        /// Conflicting relation name.
        name: String,
        /// Previously declared arity.
        expected: usize,
        /// Conflicting arity.
        actual: usize,
    },
    /// Variable is absent from all positive atoms in its clause.
    UnboundVariable(String),
    /// Relation name is empty.
    EmptyRelation,
    /// A relation depends negatively on itself, directly or indirectly.
    NegativeCycle,
    /// Evaluation was interrupted by the cancellation callback.
    Cancelled,
}

/// A failure and its zero-based location in the supplied clauses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    /// Clause or tuple associated with the failure.
    pub location: Location,
    /// The violated constraint or cancellation.
    pub kind: ErrorKind,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ErrorKind::Arity { expected, actual } => {
                write!(f, "expected {expected} terms, found {actual}")
            }
            ErrorKind::RelationArity {
                name,
                expected,
                actual,
            } => write!(
                f,
                "relation `{name}` has arity {actual}, expected {expected}"
            ),
            ErrorKind::UnboundVariable(name) => {
                write!(f, "variable `{name}` has no positive binding")
            }
            ErrorKind::EmptyRelation => f.write_str("relation name must not be empty"),
            ErrorKind::NegativeCycle => f.write_str("negation creates a recursive dependency"),
            ErrorKind::Cancelled => f.write_str("query cancelled"),
        }
    }
}
impl std::error::Error for Error {}

/// An immutable database snapshot; replacing facts requires a new snapshot.
#[derive(Clone, Debug)]
pub struct Database<C> {
    pub(crate) snapshot: Arc<Snapshot<C>>,
}

#[derive(Debug)]
pub(crate) struct Snapshot<C> {
    pub(crate) facts: Vec<(Relation, Vec<C>)>,
    pub(crate) rules: Vec<Rule<C>>,
    pub(crate) arities: BTreeMap<String, usize>,
    pub(crate) strata: BTreeMap<String, usize>,
    pub(crate) relations: BTreeMap<String, RelationIndex<C>>,
    pub(crate) rule_heads: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug)]
pub(crate) struct RelationIndex<C> {
    pub(crate) rows: Vec<usize>,
    pub(crate) columns: Vec<BTreeMap<C, Vec<usize>>>,
}

impl<C: PartialEq> PartialEq for Database<C> {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.facts == other.snapshot.facts && self.snapshot.rules == other.snapshot.rules
    }
}
impl<C: Eq> Eq for Database<C> {}
impl<C: Hash> Hash for Database<C> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.snapshot.facts.hash(hasher);
        self.snapshot.rules.hash(hasher);
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
    fn var(name: &str) -> Term<i32> {
        Term::Variable(name.into())
    }
    fn constant(value: i32) -> Term<i32> {
        Term::Constant(value)
    }
    fn predicate(name: &str, terms: Vec<Term<i32>>) -> Atom<i32> {
        Atom {
            relation: relation(name, terms.len()),
            terms,
        }
    }
    fn positive(name: &str, terms: Vec<Term<i32>>) -> Literal<i32> {
        Literal::Positive(predicate(name, terms))
    }
    fn negative(name: &str, terms: Vec<Term<i32>>) -> Literal<i32> {
        Literal::Negative(predicate(name, terms))
    }
    fn edge(from: i32, to: i32) -> (Relation, Vec<i32>) {
        (relation("edge", 2), vec![from, to])
    }
    fn paths() -> Vec<Rule<i32>> {
        vec![
            Rule {
                head: predicate("path", vec![var("x"), var("y")]),
                body: vec![positive("edge", vec![var("x"), var("y")])],
            },
            Rule {
                head: predicate("path", vec![var("x"), var("z")]),
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
    fn run(database: &Database<i32>, query: &Query<i32>) -> Vec<Vec<i32>> {
        database.query(query, || false).unwrap()
    }

    // Enumerate ground substitutions independently of the tabled evaluator.
    fn reference(
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
    fn recursive_queries_match_ground_substitutions() {
        let mut rules = paths();
        rules.push(Rule {
            head: predicate("indirect", vec![var("x"), var("y")]),
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
            let expected = reference(&facts, &rules, &[0, 1, 2]);
            let database = Database::new(facts, rules.clone()).unwrap();
            for source in 0..3 {
                for name in ["path", "indirect"] {
                    let actual = run(
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
    fn projection_deduplicates_repeated_variables() {
        let database =
            Database::new(vec![edge(1, 1), edge(1, 1), edge(1, 2), edge(2, 2)], vec![]).unwrap();
        assert_eq!(
            run(
                &database,
                &query(
                    &["x", "x"],
                    vec![positive("edge", vec![var("x"), var("x")])]
                )
            ),
            vec![vec![1, 1], vec![2, 2]]
        );
    }

    #[test]
    fn recursive_join_matches_ground_substitutions() {
        let mut rules = paths();
        rules[1].body[1] = positive("path", vec![var("y"), var("z")]);
        let facts = vec![edge(0, 1), edge(1, 2), edge(2, 0), edge(2, 3)];
        let expected = reference(&facts, &rules, &[0, 1, 2, 3]);
        let database = Database::new(facts, rules).unwrap();
        let actual = run(
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
    fn mutual_recursion_matches_ground_substitutions() {
        let facts: Vec<_> = [[0, 1, 2], [0, 2, 1], [1, 2, 0], [2, 1, 0]]
            .into_iter()
            .map(|tuple| (relation("triple", 3), tuple.to_vec()))
            .collect();
        let rules = vec![
            Rule {
                head: predicate("p", vec![var("x"), var("y"), var("z")]),
                body: vec![positive("triple", vec![var("x"), var("y"), var("z")])],
            },
            Rule {
                head: predicate("q", vec![var("x"), var("z"), var("y")]),
                body: vec![positive("p", vec![var("x"), var("y"), var("z")])],
            },
            Rule {
                head: predicate("p", vec![var("x"), var("y"), var("w")]),
                body: vec![
                    positive("q", vec![var("x"), var("z"), var("y")]),
                    positive("triple", vec![var("y"), var("z"), var("w")]),
                ],
            },
        ];
        let all = reference(&facts, &rules, &[0, 1, 2]);
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
        let database = Database::new(facts.clone(), rules.clone()).unwrap();
        assert_eq!(
            run(&database, &query(&["x", "y", "w"], body.clone())),
            expected
        );

        fn rename(atom: &mut Atom<i32>) {
            for term in &mut atom.terms {
                if let Term::Variable(name) = term {
                    *name = format!("renamed_{name}");
                }
            }
        }
        let mut renamed = rules;
        for rule in &mut renamed {
            rename(&mut rule.head);
            for literal in &mut rule.body {
                match literal {
                    Literal::Positive(atom) | Literal::Negative(atom) => rename(atom),
                }
            }
            rule.body.reverse();
        }
        renamed.reverse();
        let mut renamed_body = body;
        for literal in &mut renamed_body {
            match literal {
                Literal::Positive(atom) | Literal::Negative(atom) => rename(atom),
            }
        }
        renamed_body.reverse();
        let renamed_database = Database::new(facts, renamed).unwrap();
        assert_eq!(
            run(
                &renamed_database,
                &query(&["renamed_x", "renamed_y", "renamed_w"], renamed_body)
            ),
            expected
        );
    }

    #[test]
    fn nullary_rules_produce_empty_projection() {
        let rules = vec![
            Rule {
                head: predicate("seed", vec![constant(7)]),
                body: vec![],
            },
            Rule {
                head: predicate("enabled", vec![]),
                body: vec![],
            },
            Rule {
                head: predicate("ready", vec![]),
                body: vec![
                    positive("seed", vec![constant(7)]),
                    positive("enabled", vec![]),
                    negative("missing", vec![]),
                ],
            },
        ];
        let database = Database::new(vec![], rules).unwrap();
        assert_eq!(
            run(&database, &query(&[], vec![positive("ready", vec![])])),
            vec![vec![]]
        );
        assert!(run(&database, &query(&[], vec![positive("missing", vec![])])).is_empty());
        assert_eq!(run(&database, &query(&[], vec![])), vec![vec![]]);
    }

    #[test]
    fn deep_negation_uses_bounded_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let rules: Vec<_> = (1..=1401)
                    .map(|level| Rule {
                        head: predicate(&format!("level{level}"), vec![]),
                        body: vec![negative(&format!("level{}", level - 1), vec![])],
                    })
                    .collect();
                let database = Database::new(vec![(relation("level0", 0), vec![])], rules).unwrap();
                assert_eq!(
                    run(&database, &query(&[], vec![positive("level1400", vec![])])),
                    vec![vec![]]
                );
                assert!(
                    run(&database, &query(&[], vec![positive("level1401", vec![])])).is_empty()
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn snapshots_canonicalize_duplicate_clauses() {
        let first = Database::new(vec![edge(1, 2), edge(2, 3)], paths()).unwrap();
        let mut rules = paths();
        rules.reverse();
        rules.push(rules[0].clone());
        let second = Database::new(vec![edge(2, 3), edge(1, 2), edge(1, 2)], rules).unwrap();
        assert_eq!(first, second);
        use std::hash::{DefaultHasher, Hasher};
        let mut first_hash = DefaultHasher::new();
        let mut second_hash = DefaultHasher::new();
        first.hash(&mut first_hash);
        second.hash(&mut second_hash);
        assert_eq!(first_hash.finish(), second_hash.finish());
    }

    #[test]
    fn snapshots_remove_unsupported_derivations() {
        let old = Database::new(vec![edge(1, 2), edge(2, 3)], paths()).unwrap();
        let new = Database::new(vec![edge(1, 2)], paths()).unwrap();
        let q = query(&["y"], vec![positive("path", vec![constant(1), var("y")])]);
        assert_eq!(run(&old, &q), vec![vec![2], vec![3]]);
        assert_eq!(run(&new, &q), vec![vec![2]]);
    }

    #[test]
    fn validation_locates_invalid_clauses() {
        let error = Database::<i32>::new(vec![(relation("edge", 2), vec![1])], vec![]).unwrap_err();
        assert_eq!(
            error,
            Error {
                location: Location::Fact(0),
                kind: ErrorKind::Arity {
                    expected: 2,
                    actual: 1
                }
            }
        );
        let error = Database::new(
            vec![edge(1, 2)],
            vec![Rule {
                head: predicate("edge", vec![var("x")]),
                body: vec![positive("edge", vec![var("x"), constant(2)])],
            }],
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::RelationArity { .. }));
        let error = Database::<i32>::new(
            vec![],
            vec![Rule {
                head: predicate("p", vec![var("x")]),
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
        assert_eq!(error.kind, ErrorKind::EmptyRelation);
        let database = Database::<i32>::new(vec![], vec![]).unwrap();
        assert_eq!(
            database
                .query(&query(&["x"], vec![]), || false)
                .unwrap_err()
                .location,
            Location::QuerySelect(0)
        );
        assert!(
            run(
                &database,
                &query(&["x"], vec![positive("absent", vec![var("x")])])
            )
            .is_empty()
        );
    }

    #[test]
    fn negative_cycles_are_rejected() {
        let rules = vec![
            Rule {
                head: predicate("p", vec![var("x")]),
                body: vec![
                    positive("domain", vec![var("x")]),
                    negative("q", vec![var("x")]),
                ],
            },
            Rule {
                head: predicate("q", vec![var("x")]),
                body: vec![positive("p", vec![var("x")])],
            },
        ];
        assert_eq!(
            Database::<i32>::new(vec![], rules).unwrap_err().kind,
            ErrorKind::NegativeCycle
        );
    }

    #[test]
    fn bound_recursion_has_no_iteration_limit() {
        let facts: Vec<_> = (0..10_050)
            .map(|i| edge(i, i + 1))
            .chain((20_000..30_000).map(|i| edge(i, i + 1)))
            .collect();
        let database = Database::new(facts, paths()).unwrap();
        let rows = run(
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
        let database = Database::new(facts, paths()).unwrap();
        for component in 0..500 {
            let source = component * 3;
            let q = query(
                &["y"],
                vec![positive("path", vec![constant(source), var("y")])],
            );
            assert_eq!(run(&database, &q), vec![vec![source + 1], vec![source + 2]]);
        }
    }

    #[test]
    fn cancellation_returns_no_rows() {
        let database = Database::new(vec![edge(1, 2), edge(2, 3)], paths()).unwrap();
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
}
