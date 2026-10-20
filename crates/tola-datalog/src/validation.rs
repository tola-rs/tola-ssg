//! Arity, binding, and stratification checks over the rows and rules a caller supplies.
//!
//! Reporting follows the caller's own order — tuples before rules, and within a clause the atom
//! as written — so every `Location` maps back to what the caller passed. Nothing here evaluates
//! a rule.
use crate::relation::RowBuffer;
use crate::{Atom, Database, Error, ErrorKind, Literal, Location, Relation, Rule, Snapshot, Term};
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(crate) fn atom<C>(literal: &Literal<C>) -> &Atom<C> {
    match literal {
        Literal::Positive(atom) | Literal::Negative(atom) => atom,
    }
}

pub(crate) fn variables<C>(atom: &Atom<C>) -> impl Iterator<Item = &String> {
    atom.terms.iter().filter_map(|term| match term {
        Term::Variable(name) => Some(name),
        Term::Constant(_) => None,
    })
}

fn arity_error(location: Location, arity: usize, actual: usize) -> Error {
    Error {
        location,
        kind: ErrorKind::Arity {
            expected: arity,
            actual,
        },
    }
}

/// The relation arities accepted so far, optionally seeded with a snapshot's table.
pub(crate) struct DeclaredArities<'a> {
    known: Option<&'a BTreeMap<String, usize>>,
    declared: BTreeMap<String, usize>,
}

impl<'a> DeclaredArities<'a> {
    /// For building a snapshot: no arity is declared yet.
    pub(crate) fn declared() -> Self {
        Self {
            known: None,
            declared: BTreeMap::new(),
        }
    }

    /// For checking a query against a snapshot, whose table is only read.
    pub(crate) fn extending(known: &'a BTreeMap<String, usize>) -> Self {
        Self {
            known: Some(known),
            declared: BTreeMap::new(),
        }
    }

    fn arity(&self, name: &str) -> Option<usize> {
        self.declared
            .get(name)
            .or_else(|| self.known.and_then(|known| known.get(name)))
            .copied()
    }

    /// Record one use of `relation`, which supplied `actual` columns.
    ///
    /// The use is checked against the relation's own arity before any earlier declaration, so a
    /// malformed atom is reported as `Arity` rather than as a conflict with other clauses.
    pub(crate) fn declare(
        &mut self,
        relation: &Relation,
        actual: usize,
        location: Location,
    ) -> Result<(), Error> {
        if relation.name.is_empty() {
            return Err(Error {
                location,
                kind: ErrorKind::EmptyRelation,
            });
        }
        if relation.arity != actual {
            return Err(arity_error(location, relation.arity, actual));
        }
        match self.arity(&relation.name) {
            Some(expected) if expected != relation.arity => Err(Error {
                location,
                kind: ErrorKind::RelationArity {
                    name: relation.name.clone(),
                    expected,
                    actual: relation.arity,
                },
            }),
            Some(_) => Ok(()),
            None => {
                self.declared.insert(relation.name.clone(), relation.arity);
                Ok(())
            }
        }
    }

    /// The arities declared while building a snapshot.
    pub(crate) fn into_declared(self) -> BTreeMap<String, usize> {
        self.declared
    }
}

pub(crate) fn validate_body<C>(
    body: &[Literal<C>],
    arities: &mut DeclaredArities<'_>,
    location: impl Fn(usize) -> Location,
) -> Result<BTreeSet<String>, Error> {
    let mut bound = BTreeSet::new();
    for (index, literal) in body.iter().enumerate() {
        let atom = atom(literal);
        arities.declare(&atom.relation, atom.terms.len(), location(index))?;
        if matches!(literal, Literal::Positive(_)) {
            bound.extend(variables(atom).cloned());
        }
    }
    // A negative atom may name a variable that a later positive atom binds, so binding is
    // complete before absence is checked.
    for (index, literal) in body.iter().enumerate() {
        let Literal::Negative(atom) = literal else {
            continue;
        };
        for name in variables(atom) {
            if !bound.contains(name) {
                return Err(Error {
                    location: location(index),
                    kind: ErrorKind::UnboundVariable(name.clone()),
                });
            }
        }
    }
    Ok(bound)
}

/// Locate a negative atom that closes a dependency cycle, the first one in input order.
///
/// Only called once the strata refuse to settle, which needs a cycle through a negative literal:
/// starting from such an atom, the dependencies lead back to its rule's head.
fn negative_cycle_location<C>(rules: &[Rule<C>]) -> Location {
    let mut dependencies = BTreeMap::<&str, Vec<&str>>::new();
    for rule in rules {
        dependencies
            .entry(&rule.head.relation.name)
            .or_default()
            .extend(
                rule.body
                    .iter()
                    .map(|literal| atom(literal).relation.name.as_str()),
            );
    }
    for (rule_index, rule) in rules.iter().enumerate() {
        for (atom_index, literal) in rule.body.iter().enumerate() {
            let Literal::Negative(target) = literal else {
                continue;
            };
            let mut pending = vec![target.relation.name.as_str()];
            let mut visited = BTreeSet::new();
            while let Some(relation) = pending.pop() {
                if relation == rule.head.relation.name {
                    return Location::RuleBody {
                        rule: rule_index,
                        atom: atom_index,
                    };
                }
                if visited.insert(relation) {
                    pending.extend(dependencies.get(relation).into_iter().flatten().copied());
                }
            }
        }
    }
    unreachable!("a negative cycle must contain a negative literal")
}

/// The stratum of every declared relation name.
///
/// A stratum is a longest weighted dependency path, shorter than the relation count, so n+1
/// passes settle the strata unless a cycle through a negative literal keeps feeding them.
/// Relations downstream of that cycle can grow too, so the reported literal comes from
/// [`negative_cycle_location`].
fn stratify<C>(
    rules: &[Rule<C>],
    arities: &BTreeMap<String, usize>,
) -> Result<BTreeMap<String, usize>, Error> {
    let mut strata: BTreeMap<String, usize> =
        arities.keys().map(|name| (name.clone(), 0)).collect();
    for pass in 0..=arities.len() {
        let mut grew = false;
        for rule in rules {
            for literal in &rule.body {
                let required = strata[&atom(literal).relation.name]
                    + usize::from(matches!(literal, Literal::Negative(_)));
                let head = strata.get_mut(&rule.head.relation.name).unwrap();
                if *head < required {
                    if pass == arities.len() {
                        return Err(Error {
                            location: negative_cycle_location(rules),
                            kind: ErrorKind::NegativeCycle,
                        });
                    }
                    *head = required;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    Ok(strata)
}

/// Collect rows into their relation buffers and validate them in caller order.
///
/// Grouping by the whole relation keeps two arities of one name apart, so the conflict surfaces at
/// the first row that uses the later arity. Each source stays alive until its values are pushed.
fn collect_facts<C, I, R, T>(
    rows: I,
    arities: &mut DeclaredArities<'_>,
) -> Result<Vec<(Relation, RowBuffer<C>)>, Error>
where
    I: IntoIterator<Item = (R, T)>,
    R: Borrow<Relation>,
    T: IntoIterator<Item = C>,
{
    let mut groups = Vec::<(Relation, RowBuffer<C>)>::new();
    let mut values_per_group = Vec::<usize>::new();
    let mut group_of_relation = BTreeMap::<Relation, usize>::new();
    let rows = rows.into_iter();
    let mut row_sources = Vec::with_capacity(rows.size_hint().0);
    for (source, values) in rows {
        let relation = source.borrow();
        let values = values.into_iter();
        let values_hint = values.size_hint().0;
        let group = match group_of_relation.get(relation) {
            Some(&group) => group,
            None => {
                let group = groups.len();
                group_of_relation.insert(relation.clone(), group);
                groups.push((relation.clone(), RowBuffer::new(relation.arity)));
                values_per_group.push(0);
                group
            }
        };
        values_per_group[group] += values_hint;
        row_sources.push((group, source, values));
    }
    row_sources.shrink_to_fit();
    for ((_, buffer), values) in groups.iter_mut().zip(values_per_group) {
        buffer.reserve(values);
    }
    for (index, (group, _source, values)) in row_sources.into_iter().enumerate() {
        let (relation, buffer) = &mut groups[group];
        let actual = buffer.push_row(values);
        arities.declare(relation, actual, Location::Fact(index))?;
    }
    Ok(groups)
}

/// Declare rule heads and validate their bodies, in input order.
fn declare_rules<C>(rules: &[Rule<C>], arities: &mut DeclaredArities<'_>) -> Result<(), Error> {
    for (index, rule) in rules.iter().enumerate() {
        arities.declare(
            &rule.head.relation,
            rule.head.terms.len(),
            Location::RuleHead(index),
        )?;
        let bound = validate_body(&rule.body, arities, |atom| Location::RuleBody {
            rule: index,
            atom,
        })?;
        for name in variables(&rule.head) {
            if !bound.contains(name) {
                return Err(Error {
                    location: Location::RuleHead(index),
                    kind: ErrorKind::UnboundVariable(name.clone()),
                });
            }
        }
    }
    Ok(())
}

impl<C: Ord> Database<C> {
    /// Validate ground tuples and rules and build an immutable snapshot.
    ///
    /// Errors point at the first invalid input in the caller's order: tuples before rules.
    /// Negative dependencies must be stratified, or the clause closing a cycle is reported as
    /// `NegativeCycle` at its negative atom.
    pub fn new(facts: Vec<(Relation, Vec<C>)>, rules: Vec<Rule<C>>) -> Result<Self, Error> {
        Self::from_row_sources(facts, rules)
    }

    /// Construct a snapshot from rows whose relation may be borrowed.
    ///
    /// Each row source outlives the values it yields. Errors are located as in [`Database::new`].
    pub fn from_rows<I, R, T>(rows: I, rules: Vec<Rule<C>>) -> Result<Self, Error>
    where
        I: IntoIterator<Item = (R, T)>,
        R: Borrow<Relation>,
        T: IntoIterator<Item = C>,
    {
        Self::from_row_sources(rows, rules)
    }

    fn from_row_sources<I, R, T>(rows: I, mut rules: Vec<Rule<C>>) -> Result<Self, Error>
    where
        I: IntoIterator<Item = (R, T)>,
        R: Borrow<Relation>,
        T: IntoIterator<Item = C>,
    {
        let mut arities = DeclaredArities::declared();
        let groups = collect_facts(rows, &mut arities)?;
        declare_rules(&rules, &mut arities)?;
        let arities = arities.into_declared();
        let strata = stratify(&rules, &arities)?;

        // Canonicalize after validation, so rejected inputs pay no sorting cost.
        let relations = groups
            .into_iter()
            .map(|(relation, buffer)| (relation.name, buffer.canonicalize()))
            .collect();
        rules.sort();
        rules.dedup();
        let rules: Vec<_> = rules
            .into_iter()
            .map(|rule| {
                let program = crate::evaluation::Program::rule(&rule);
                crate::evaluation::RuleProgram { rule, program }
            })
            .collect();
        let mut rules_by_head = BTreeMap::<String, Vec<usize>>::new();
        for (index, compiled) in rules.iter().enumerate() {
            rules_by_head
                .entry(compiled.rule.head.relation.name.clone())
                .or_default()
                .push(index);
        }
        Ok(Self {
            snapshot: Arc::new(Snapshot {
                rules,
                arities,
                strata,
                relations,
                rules_by_head,
            }),
        })
    }
}
