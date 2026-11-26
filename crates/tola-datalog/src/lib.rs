#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! In-memory Datalog with demand-driven evaluation and stratified negation.
//!
//! ```
//! use tola_datalog::{Atom, Database, Literal, Query, Relation, Term};
//! let edge = Relation { name: "edge".into(), arity: 2 };
//! let db = Database::new(vec![edge.clone()], vec![(edge.clone(), vec![1, 2])], vec![])?;
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

/// Named relation with a fixed column count.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Relation {
    /// Non-empty relation identifier; all uses must agree on `arity`.
    pub name: String,
    /// Column count (zero denotes a proposition).
    pub arity: usize,
}

/// Variable local to one clause or a ground constant value.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Term<C> {
    /// Variable matched by name within the enclosing clause.
    Variable(String),
    /// Ground constant matched by equality.
    Constant(C),
}

/// Relation applied to one term per column.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Atom<C> {
    /// Target relation descriptor.
    pub relation: Relation,
    /// Terms in column order.
    pub terms: Vec<Term<C>>,
}

/// Positive match or absence of a ground match.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Literal<C> {
    /// Holds when at least one tuple matches.
    Positive(Atom<C>),
    /// Holds when the ground tuple is absent.
    Negative(Atom<C>),
}

/// Inference rule deriving its head from all body literals.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Rule<C> {
    /// Head atom derived over variables bound by the body.
    pub head: Atom<C>,
    /// Conjunctive body literals.
    pub body: Vec<Literal<C>>,
}

/// Conjunctive query with an ordered variable projection.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Query<C> {
    /// Output variables in column order (duplicates allowed).
    pub select: Vec<String>,
    /// Conjunctive body literals.
    pub body: Vec<Literal<C>>,
}

/// Zero-based caller input position where an error occurred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Location {
    /// Index into the relation declarations.
    Relation(usize),
    /// Index into the input facts across all relations.
    Fact(usize),
    /// Index into the `rules` argument for a rule head error.
    RuleHead(usize),
    /// Rule index and body atom index for a rule body error.
    RuleBody {
        /// Index into the `rules` argument.
        rule: usize,
        /// Index into that rule's body.
        atom: usize,
    },
    /// Index into the query body.
    QueryBody(usize),
    /// Index into the query `select` list.
    QuerySelect(usize),
    /// Whole-database error such as cancellation.
    Database,
}

/// Validation or evaluation failure cause.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// Atom or fact supplied a different term count than the relation's arity.
    Arity {
        /// Declared arity.
        expected: usize,
        /// Supplied term count.
        actual: usize,
    },
    /// Relation use disagrees with the declared arity.
    RelationArity {
        /// Relation name.
        name: String,
        /// Declared arity.
        expected: usize,
        /// Conflicting arity.
        actual: usize,
    },
    /// Relation name was declared more than once.
    DuplicateRelation(String),
    /// Relation name has no declaration.
    UnknownRelation(String),
    /// Rule has an empty body.
    EmptyBody,
    /// Variable name is empty.
    EmptyVariable,
    /// Variable is not bound by any positive atom in the clause.
    UnboundVariable(String),
    /// Relation name is empty.
    EmptyRelation,
    /// Negation forms a cyclic dependency.
    NegativeCycle,
    /// Cancellation predicate returned `true` during evaluation.
    Cancelled,
}

/// Validation or evaluation failure paired with its caller input location.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{}", kind_message(.kind))]
pub struct Error {
    /// Position in caller input where the error occurred.
    pub location: Location,
    /// Failure cause.
    pub kind: ErrorKind,
}

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
        ErrorKind::DuplicateRelation(name) => format!("relation `{name}` is already declared"),
        ErrorKind::UnknownRelation(name) => format!("relation `{name}` is not declared"),
        ErrorKind::EmptyBody => "a rule needs at least one body literal".to_owned(),
        ErrorKind::EmptyVariable => "variable name must not be empty".to_owned(),
        ErrorKind::UnboundVariable(name) => format!("variable `{name}` has no positive binding"),
        ErrorKind::EmptyRelation => "relation name must not be empty".to_owned(),
        ErrorKind::NegativeCycle => "negation creates a recursive dependency".to_owned(),
        ErrorKind::Cancelled => "query cancelled".to_owned(),
    }
}

/// Immutable snapshot of validated facts and rules; `Clone` shares storage.
#[derive(Debug)]
pub struct Database<C> {
    pub(crate) snapshot: Arc<Snapshot<C>>,
}

impl<C> Clone for Database<C> {
    // Manual impl avoids requiring `C: Clone`.
    fn clone(&self) -> Self {
        Self {
            snapshot: self.snapshot.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Snapshot<C> {
    pub(crate) rules: Vec<evaluation::RuleProgram<C>>,
    pub(crate) arities: BTreeMap<String, usize>,
    pub(crate) strata: BTreeMap<String, usize>,
    pub(crate) relations: BTreeMap<String, relation::RelationRows<C>>,
    pub(crate) rules_by_head: BTreeMap<String, Vec<usize>>,
}

impl<C: PartialEq> PartialEq for Database<C> {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.arities == other.snapshot.arities
            && self.snapshot.relations == other.snapshot.relations
            && self.snapshot.rules.iter().map(|c| &c.rule).eq(other
                .snapshot
                .rules
                .iter()
                .map(|c| &c.rule))
    }
}

impl<C: Eq> Eq for Database<C> {}

impl<C: Hash> Hash for Database<C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.snapshot.arities.hash(state);
        self.snapshot.relations.hash(state);
        self.snapshot.rules.len().hash(state);
        for c in &self.snapshot.rules {
            c.rule.hash(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::{atom, variables};
    use std::collections::BTreeSet;

    fn rel(name: &str, arity: usize) -> Relation {
        Relation {
            name: name.into(),
            arity,
        }
    }
    fn var<C>(name: &str) -> Term<C> {
        Term::Variable(name.into())
    }
    fn val(v: i32) -> Term<i32> {
        Term::Constant(v)
    }
    fn atom_of<C>(name: &str, terms: Vec<Term<C>>) -> Atom<C> {
        Atom {
            relation: rel(name, terms.len()),
            terms,
        }
    }
    fn pos<C>(name: &str, terms: Vec<Term<C>>) -> Literal<C> {
        Literal::Positive(atom_of(name, terms))
    }
    fn neg<C>(name: &str, terms: Vec<Term<C>>) -> Literal<C> {
        Literal::Negative(atom_of(name, terms))
    }
    fn edge(from: i32, to: i32) -> (Relation, Vec<i32>) {
        (rel("edge", 2), vec![from, to])
    }
    fn paths() -> Vec<Rule<i32>> {
        vec![
            Rule {
                head: atom_of("path", vec![var("x"), var("y")]),
                body: vec![pos("edge", vec![var("x"), var("y")])],
            },
            Rule {
                head: atom_of("path", vec![var("x"), var("z")]),
                body: vec![
                    pos("path", vec![var("x"), var("y")]),
                    pos("edge", vec![var("y"), var("z")]),
                ],
            },
        ]
    }
    fn query(select: &[&str], body: Vec<Literal<i32>>) -> Query<i32> {
        Query {
            select: select.iter().map(|s| (*s).into()).collect(),
            body,
        }
    }
    fn atom_query<C>(select: &[&str], r: &Relation, terms: Vec<Term<C>>) -> Query<C> {
        Query {
            select: select.iter().map(|s| (*s).into()).collect(),
            body: vec![Literal::Positive(Atom {
                relation: r.clone(),
                terms,
            })],
        }
    }
    fn eval(db: &Database<i32>, q: &Query<i32>) -> Vec<Vec<i32>> {
        db.query(q, || false).unwrap()
    }
    fn build_db<C: Ord>(facts: Vec<(Relation, Vec<C>)>, rules: Vec<Rule<C>>) -> Database<C> {
        let rels = facts
            .iter()
            .map(|(r, _)| r)
            .chain(rules.iter().flat_map(|r| {
                std::iter::once(&r.head.relation).chain(r.body.iter().map(|l| &atom(l).relation))
            }))
            .cloned()
            .collect::<BTreeSet<_>>();
        Database::from_rows(rels, facts, rules).unwrap()
    }

    // Naive bottom-up ground reference evaluator for cross-checking tabled evaluation.
    fn reference_answers(
        facts: &[(Relation, Vec<i32>)],
        rules: &[Rule<i32>],
        domain: &[i32],
    ) -> BTreeSet<(String, Vec<i32>)> {
        let mut tups: BTreeSet<_> = facts
            .iter()
            .map(|(r, tup)| (r.name.clone(), tup.clone()))
            .collect();
        let mut strata = BTreeMap::<String, usize>::new();
        for rule in rules {
            strata.entry(rule.head.relation.name.clone()).or_default();
            for lit in &rule.body {
                strata.entry(atom(lit).relation.name.clone()).or_default();
            }
        }
        loop {
            let mut changed = false;
            for rule in rules {
                for lit in &rule.body {
                    let needed = strata[&atom(lit).relation.name]
                        + usize::from(matches!(lit, Literal::Negative(_)));
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
                    .filter(|r| strata[&r.head.relation.name] == level)
                {
                    let vars: Vec<_> = variables(&rule.head)
                        .chain(rule.body.iter().flat_map(|l| variables(atom(l))))
                        .cloned()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    let count = domain.len().pow(vars.len() as u32);
                    for mut idx in 0..count {
                        let mut env = BTreeMap::new();
                        for v in &vars {
                            env.insert(v.clone(), domain[idx % domain.len()]);
                            idx /= domain.len();
                        }
                        let ground = |a: &Atom<i32>| {
                            a.terms
                                .iter()
                                .map(|t| match t {
                                    Term::Constant(v) => *v,
                                    Term::Variable(v) => env[v],
                                })
                                .collect::<Vec<_>>()
                        };
                        if rule.body.iter().all(|lit| {
                            let present = tups
                                .contains(&(atom(lit).relation.name.clone(), ground(atom(lit))));
                            present == matches!(lit, Literal::Positive(_))
                        }) {
                            added.push((rule.head.relation.name.clone(), ground(&rule.head)));
                        }
                    }
                }
                let before = tups.len();
                tups.extend(added);
                if before == tups.len() {
                    break;
                }
            }
        }
        tups
    }

    #[test]
    fn recursive_closure() {
        let mut rules = paths();
        rules.push(Rule {
            head: atom_of("indirect", vec![var("x"), var("y")]),
            body: vec![
                neg("edge", vec![var("x"), var("y")]),
                pos("path", vec![var("x"), var("y")]),
            ],
        });
        for mask in 0..512 {
            let facts: Vec<_> = (0..9)
                .filter(|b| mask & (1 << b) != 0)
                .map(|b| edge(b / 3, b % 3))
                .chain((0..3).map(|n| (rel("node", 1), vec![n])))
                .collect();
            let expected = reference_answers(&facts, &rules, &[0, 1, 2]);
            let db = build_db(facts, rules.clone());
            for src in 0..3 {
                let missing: Vec<_> = (0..3)
                    .map(|dst| vec![dst])
                    .filter(|dst| !expected.contains(&("path".into(), vec![src, dst[0]])))
                    .collect();
                assert_eq!(
                    eval(
                        &db,
                        &query(
                            &["y"],
                            vec![
                                pos("node", vec![var("y")]),
                                neg("path", vec![val(src), var("y")]),
                            ],
                        ),
                    ),
                    missing,
                    "mask {mask}, missing from {src}"
                );
                for name in ["path", "indirect"] {
                    let actual = eval(
                        &db,
                        &query(&["y"], vec![pos(name, vec![val(src), var("y")])]),
                    );
                    let expected_rows: Vec<_> = expected
                        .iter()
                        .filter(|(r, tup)| r == name && tup[0] == src)
                        .map(|(_, tup)| vec![tup[1]])
                        .collect();
                    assert_eq!(actual, expected_rows, "mask {mask}, {name}, src {src}");
                }
            }
        }
    }

    #[test]
    fn projection_sorted_distinct() {
        let db = build_db(
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
                query(&["x", "x"], vec![pos("edge", vec![var("x"), var("x")])]),
                vec![vec![1, 1], vec![2, 2]],
            ),
            (
                query(&["y", "x"], vec![pos("edge", vec![var("x"), var("y")])]),
                vec![vec![0, 2], vec![1, 1], vec![2, 0], vec![2, 1], vec![2, 2]],
            ),
            (
                query(&["y"], vec![pos("edge", vec![var("x"), var("y")])]),
                vec![vec![0], vec![1], vec![2]],
            ),
            (
                query(&[], vec![pos("edge", vec![var("x"), var("y")])]),
                vec![vec![]],
            ),
        ];
        for (q, expected) in cases {
            assert_eq!(eval(&db, &q), expected);
        }
    }

    #[test]
    fn recursive_self_join_cycle() {
        let mut rules = paths();
        rules[1].body[1] = pos("path", vec![var("y"), var("z")]);
        let facts = vec![edge(0, 1), edge(1, 2), edge(2, 0), edge(2, 3)];
        let expected = reference_answers(&facts, &rules, &[0, 1, 2, 3]);
        let db = build_db(facts, rules);
        let actual = eval(
            &db,
            &query(&["x", "z"], vec![pos("path", vec![var("x"), var("z")])]),
        );
        let expected: Vec<_> = expected
            .into_iter()
            .filter(|(name, _)| name == "path")
            .map(|(_, tup)| tup)
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn mutual_recursion_invariance() {
        let facts: Vec<_> = [[0, 1, 2], [0, 2, 1], [1, 2, 0], [2, 1, 0]]
            .into_iter()
            .map(|tup| (rel("triple", 3), tup.to_vec()))
            .collect();
        let rules = vec![
            Rule {
                head: atom_of("p", vec![var("x"), var("y"), var("z")]),
                body: vec![pos("triple", vec![var("x"), var("y"), var("z")])],
            },
            Rule {
                head: atom_of("q", vec![var("x"), var("z"), var("y")]),
                body: vec![pos("p", vec![var("x"), var("y"), var("z")])],
            },
            Rule {
                head: atom_of("p", vec![var("x"), var("y"), var("w")]),
                body: vec![
                    pos("q", vec![var("x"), var("z"), var("y")]),
                    pos("triple", vec![var("y"), var("z"), var("w")]),
                ],
            },
        ];
        let all = reference_answers(&facts, &rules, &[0, 1, 2]);
        let tups: Vec<_> = all
            .iter()
            .filter(|(name, _)| name == "p")
            .map(|(_, tup)| tup)
            .collect();
        let expected: Vec<_> = tups
            .iter()
            .flat_map(|l| {
                tups.iter()
                    .filter(move |r| l[0] == r[0] && l[2] == r[1])
                    .map(move |r| vec![l[0], l[1], r[2]])
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let body = vec![
            pos("p", vec![var("x"), var("y"), var("z")]),
            pos("p", vec![var("x"), var("z"), var("w")]),
        ];
        let db = build_db(facts.clone(), rules.clone());
        assert_eq!(eval(&db, &query(&["x", "y", "w"], body.clone())), expected);

        fn rename_atom(a: &mut Atom<i32>) {
            for t in &mut a.terms {
                if let Term::Variable(v) = t {
                    *v = format!("renamed_{v}");
                }
            }
        }
        fn rename_lit(lit: &mut Literal<i32>) {
            match lit {
                Literal::Positive(a) | Literal::Negative(a) => rename_atom(a),
            }
        }
        let mut renamed = rules;
        for rule in &mut renamed {
            rename_atom(&mut rule.head);
            for lit in &mut rule.body {
                rename_lit(lit);
            }
            rule.body.reverse();
        }
        renamed.reverse();
        let mut renamed_body = body;
        for lit in &mut renamed_body {
            rename_lit(lit);
        }
        renamed_body.reverse();
        let renamed_db = build_db(facts, renamed);
        assert_eq!(
            eval(
                &renamed_db,
                &query(&["renamed_x", "renamed_y", "renamed_w"], renamed_body)
            ),
            expected
        );
    }

    #[test]
    fn nullary_relations() {
        let rules = vec![Rule {
            head: atom_of("ready", vec![]),
            body: vec![
                pos("seed", vec![val(7)]),
                pos("enabled", vec![]),
                neg("missing", vec![]),
            ],
        }];
        let db = build_db(
            vec![(rel("seed", 1), vec![7]), (rel("enabled", 0), vec![])],
            rules,
        );
        assert_eq!(
            eval(&db, &query(&[], vec![pos("ready", vec![])])),
            vec![vec![]]
        );
        assert!(eval(&db, &query(&[], vec![pos("missing", vec![])])).is_empty());
        assert_eq!(eval(&db, &query(&[], vec![])), vec![vec![]]);
    }

    #[test]
    fn deep_negation_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let rules: Vec<_> = (1..=1401)
                    .map(|lvl| Rule {
                        head: atom_of(&format!("level{lvl}"), vec![]),
                        body: vec![neg(&format!("level{}", lvl - 1), vec![])],
                    })
                    .collect();
                let db = build_db(vec![(rel("level0", 0), vec![])], rules);
                assert_eq!(
                    eval(&db, &query(&[], vec![pos("level1400", vec![])])),
                    vec![vec![]]
                );
                assert!(eval(&db, &query(&[], vec![pos("level1401", vec![])])).is_empty());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn snapshot_order_invariance() {
        let rules = paths();
        let a = build_db(vec![edge(1, 2), edge(2, 3)], rules.clone());
        let mut reordered = rules;
        reordered.reverse();
        reordered.push(reordered[0].clone());
        let b = build_db(vec![edge(2, 3), edge(1, 2), edge(1, 2)], reordered);
        assert_eq!(a, b);
        use std::hash::DefaultHasher;
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    #[test]
    fn from_rows_arity_error() {
        let r = rel("value", usize::MAX);
        let err = Database::<i32>::from_rows([&r], [(&r, [])], vec![]).unwrap_err();
        assert_eq!(
            err,
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
    fn from_rows_dedup_first() {
        let r = rel("value", 1);
        let first: Arc<str> = Arc::from("same");
        let dup: Arc<str> = Arc::from("same");
        let rows = [Arc::from("later"), first.clone(), dup];
        let db = Database::from_rows([&r], rows.into_iter().map(|v| (&r, [v])), vec![]).unwrap();
        let q = atom_query(&["x"], &r, vec![var("x")]);
        let rows = db.query(&q, || false).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(Arc::ptr_eq(&rows[1][0], &first));
    }

    #[test]
    fn snapshot_isolation() {
        let rules = paths();
        let old = build_db(vec![edge(1, 2), edge(2, 3)], rules.clone());
        let new = build_db(vec![edge(1, 2)], rules);
        let q = query(&["y"], vec![pos("path", vec![val(1), var("y")])]);
        assert_eq!(eval(&old, &q), vec![vec![2], vec![3]]);
        assert_eq!(eval(&new, &q), vec![vec![2]]);
    }

    #[test]
    fn validation_locations() {
        let err = Database::new(
            vec![rel("edge", 2)],
            vec![edge(1, 2)],
            vec![Rule {
                head: atom_of("edge", vec![var("x")]),
                body: vec![pos("edge", vec![var("x"), val(2)])],
            }],
        )
        .unwrap_err();
        assert_eq!(
            err,
            Error {
                location: Location::RuleHead(0),
                kind: ErrorKind::RelationArity {
                    name: "edge".into(),
                    expected: 2,
                    actual: 1
                }
            }
        );
        let err = Database::<i32>::new(
            vec![rel("p", 1), rel("q", 1)],
            vec![],
            vec![Rule {
                head: atom_of("p", vec![var("x")]),
                body: vec![neg("q", vec![var("x")])],
            }],
        )
        .unwrap_err();
        assert_eq!(
            err,
            Error {
                location: Location::RuleBody { rule: 0, atom: 0 },
                kind: ErrorKind::UnboundVariable("x".into())
            }
        );
        let err = Database::<i32>::new(vec![rel("", 0)], vec![], vec![]).unwrap_err();
        assert_eq!(
            err,
            Error {
                location: Location::Relation(0),
                kind: ErrorKind::EmptyRelation
            }
        );
        let db = build_db(vec![], vec![]);
        assert_eq!(
            db.query(&query(&["x"], vec![]), || false)
                .unwrap_err()
                .location,
            Location::QuerySelect(0)
        );
    }

    #[test]
    fn validation_first_body_atom() {
        let bad = Literal::Positive(Atom {
            relation: rel("wrong", 1),
            terms: vec![],
        });
        let db = Database::<i32>::new(vec![rel("q", 1)], vec![], vec![]).unwrap();
        let q = query(&[], vec![neg("q", vec![var("x")]), bad]);
        let err = db.query(&q, || false).unwrap_err();
        assert_eq!(err.location, Location::QueryBody(0));
        assert_eq!(err.kind, ErrorKind::UnboundVariable("x".into()));
    }

    #[test]
    fn existential_tabled_join() {
        let mut facts = vec![
            (rel("pair", 2), vec![1, 2]),
            (rel("input", 1), vec![10]),
            (rel("input", 1), vec![20]),
            (rel("exclude", 1), vec![20]),
        ];
        let rules = vec![Rule {
            head: atom_of("pair", vec![var("y"), var("y")]),
            body: vec![pos("diagonal", vec![var("y")])],
        }];
        let q = query(
            &["x"],
            vec![
                pos("input", vec![var("x")]),
                pos("pair", vec![var("y"), var("y")]),
                neg("exclude", vec![var("x")]),
            ],
        );
        let before = build_db(facts.clone(), rules.clone());
        assert!(eval(&before, &q).is_empty());
        facts.push((rel("diagonal", 1), vec![3]));
        let after = build_db(facts, rules);
        assert_eq!(eval(&after, &q), vec![vec![10]]);
    }

    #[test]
    fn from_rows_drains_before_drop() {
        use std::borrow::Borrow;
        use std::cell::Cell;
        use std::rc::Rc;

        struct Source {
            rel: Relation,
            dropped: Rc<Cell<bool>>,
        }
        impl Borrow<Relation> for Source {
            fn borrow(&self) -> &Relation {
                &self.rel
            }
        }
        impl Drop for Source {
            fn drop(&mut self) {
                self.dropped.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let observed = dropped.clone();
        let r = rel("value", 1);
        let src = Source {
            rel: r.clone(),
            dropped: dropped.clone(),
        };
        let vals = std::iter::once_with(move || if observed.get() { 2 } else { 1 });
        let db = Database::from_rows([&r], [(src, vals)], vec![]).unwrap();
        assert!(dropped.get());
        let q = atom_query(&["x"], &r, vec![var("x")]);
        assert_eq!(eval(&db, &q), vec![vec![1]]);
    }

    #[test]
    fn declarations_validate_in_order() {
        let cases = [
            (
                vec![(rel("p", 2), vec![1]), (rel("", 0), vec![])],
                Location::Fact(0),
                ErrorKind::Arity {
                    expected: 2,
                    actual: 1,
                },
            ),
            (
                vec![(rel("p", 2), vec![1, 2]), (rel("p", 1), vec![1])],
                Location::Fact(1),
                ErrorKind::RelationArity {
                    name: "p".into(),
                    expected: 2,
                    actual: 1,
                },
            ),
            (
                vec![(rel("p", 2), vec![1, 2]), (rel("p", 1), vec![])],
                Location::Fact(1),
                ErrorKind::Arity {
                    expected: 1,
                    actual: 0,
                },
            ),
        ];
        for (rows, location, kind) in cases {
            assert_eq!(
                Database::new(vec![rel("p", 2)], rows, vec![]).unwrap_err(),
                Error { location, kind }
            );
        }
    }

    #[test]
    fn negative_cycle_location() {
        for self_cycle in [false, true] {
            for neg_first in [false, true] {
                for reverse in [false, true] {
                    let mut rules = vec![
                        Rule {
                            head: atom_of("downstream", vec![var("x")]),
                            body: vec![pos("p", vec![var("x")])],
                        },
                        Rule {
                            head: atom_of("outside", vec![var("x")]),
                            body: vec![pos("domain", vec![var("x")]), neg("p", vec![var("x")])],
                        },
                        Rule {
                            head: atom_of("p", vec![var("x")]),
                            body: vec![
                                pos("domain", vec![var("x")]),
                                neg(if self_cycle { "p" } else { "q" }, vec![var("x")]),
                            ],
                        },
                    ];
                    if !self_cycle {
                        rules.push(Rule {
                            head: atom_of("q", vec![var("x")]),
                            body: vec![pos("p", vec![var("x")])],
                        });
                    }
                    if neg_first {
                        rules.swap(0, 1);
                    }
                    if reverse {
                        rules.reverse();
                    }
                    let rule = rules
                        .iter()
                        .position(|r| r.head.relation.name == "p")
                        .unwrap();
                    assert_eq!(
                        Database::<i32>::new(
                            rules
                                .iter()
                                .map(|r| r.head.relation.clone())
                                .chain([rel("domain", 1)])
                                .collect(),
                            vec![],
                            rules,
                        )
                        .unwrap_err(),
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
    fn negation_waits_for_recursion() {
        let mut tups = vec![edge(0, 1), edge(1, 2)];
        tups.extend((0..3).map(|id| (rel("node", 1), vec![id])));
        let mut rules = paths();
        rules.extend([
            Rule {
                head: atom_of("missing", vec![var("x")]),
                body: vec![
                    pos("node", vec![var("x")]),
                    neg("path", vec![var("x"), val(2)]),
                ],
            },
            Rule {
                head: atom_of("present", vec![var("x")]),
                body: vec![pos("node", vec![var("x")]), neg("missing", vec![var("x")])],
            },
        ]);
        let db = build_db(tups, rules);
        assert_eq!(
            eval(&db, &query(&["x"], vec![pos("present", vec![var("x")])])),
            vec![vec![0], vec![1]]
        );
    }

    #[test]
    fn negative_descending_keys() {
        let cand = rel("candidate", 2);
        let facts = vec![
            edge(0, 1),
            edge(1, 2),
            (cand.clone(), vec![0, 3]),
            (cand.clone(), vec![1, 2]),
            (cand, vec![2, 1]),
        ];
        let db = build_db(facts, paths());
        let q = query(
            &["id"],
            vec![
                pos("candidate", vec![var("id"), var("target")]),
                neg("path", vec![val(0), var("target")]),
            ],
        );
        assert_eq!(eval(&db, &q), vec![vec![0]]);
    }

    #[test]
    fn repeated_head_var() {
        let db = build_db(
            vec![(rel("node", 1), vec![1]), (rel("node", 1), vec![2])],
            vec![Rule {
                head: atom_of("same", vec![var("x"), var("x")]),
                body: vec![pos("node", vec![var("x")])],
            }],
        );
        assert_eq!(
            eval(
                &db,
                &query(&["y"], vec![pos("same", vec![val(1), var("y")])])
            ),
            vec![vec![1]]
        );
        assert!(eval(&db, &query(&[], vec![pos("same", vec![val(1), val(2)])])).is_empty());
    }

    #[test]
    fn projection_dedup_first() {
        let pair = rel("pair", 2);
        let vals: Vec<Arc<str>> = (0..64).map(|i| Arc::from((i % 3).to_string())).collect();
        let tups = vals
            .iter()
            .enumerate()
            .map(|(i, v)| (pair.clone(), vec![Arc::from(format!("{i:02}")), v.clone()]))
            .collect();
        let db = build_db(tups, vec![]);
        let q = atom_query(&["value"], &pair, vec![var("key"), var("value")]);
        let rows = db.query(&q, || false).unwrap();
        assert_eq!(rows.len(), 3);
        for (row, first) in rows.iter().zip(&vals) {
            assert!(Arc::ptr_eq(&row[0], first));
        }
    }

    #[test]
    fn unbounded_recursion() {
        let facts: Vec<_> = (0..10_050)
            .map(|i| edge(i, i + 1))
            .chain((20_000..30_000).map(|i| edge(i, i + 1)))
            .collect();
        let db = build_db(facts, paths());
        let rows = eval(
            &db,
            &query(&["y"], vec![pos("path", vec![val(0), var("y")])]),
        );
        assert_eq!(rows.len(), 10_050);
        assert_eq!(rows.last(), Some(&vec![10_050]));
    }

    #[test]
    fn repeated_bound_queries() {
        let facts = (0..500)
            .flat_map(|c| {
                let s = c * 3;
                [edge(s, s + 1), edge(s + 1, s + 2)]
            })
            .collect();
        let db = build_db(facts, paths());
        for c in 0..500 {
            let s = c * 3;
            let q = query(&["y"], vec![pos("path", vec![val(s), var("y")])]);
            assert_eq!(eval(&db, &q), vec![vec![s + 1], vec![s + 2]]);
        }
    }

    #[test]
    fn query_cancel() {
        let db = build_db(vec![edge(1, 2), edge(2, 3)], paths());
        let mut calls = 0;
        let err = db
            .query(
                &query(&["y"], vec![pos("path", vec![val(1), var("y")])]),
                || {
                    calls += 1;
                    calls > 8
                },
            )
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Cancelled);
    }

    #[test]
    fn materialize_cancel() {
        use std::cell::Cell;

        #[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
        struct Key<'a> {
            ord: u8,
            cancelled: &'a Cell<bool>,
        }
        impl Clone for Key<'_> {
            fn clone(&self) -> Self {
                self.cancelled.set(true);
                Self {
                    ord: self.ord,
                    cancelled: self.cancelled,
                }
            }
        }

        for count in [1, 2] {
            let cancelled = Cell::new(false);
            let r = rel("key", 1);
            let facts = (1..=count)
                .map(|ord| {
                    (
                        r.clone(),
                        vec![Key {
                            ord,
                            cancelled: &cancelled,
                        }],
                    )
                })
                .collect();
            let db = build_db(facts, vec![]);
            let q = atom_query(&["key"], &r, vec![var("key")]);
            let err = db.query(&q, || cancelled.get()).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Cancelled);
        }
    }
}
