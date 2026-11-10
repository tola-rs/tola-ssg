//! Arity, binding, and stratification checks over the rows and rules a caller supplies.
//!
//! Reporting follows the caller's own order — tuples before rules, and within a clause the atom
//! as written — so every `Location` maps back to what the caller passed. Nothing here evaluates
//! a rule.
use crate::relation::RowBuffer;
use crate::{Atom, Database, Error, ErrorKind, Literal, Location, Relation, Rule, Snapshot, Term};
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// Positive literals bind variables regardless of where the literals appear in the body.
pub(crate) fn bound_variables<C>(body: &[Literal<C>]) -> BTreeSet<&String> {
    body.iter()
        .filter_map(|literal| match literal {
            Literal::Positive(atom) => Some(atom),
            Literal::Negative(_) => None,
        })
        .flat_map(variables)
        .collect()
}

fn unbound_variable<'a, C>(atom: &'a Atom<C>, bound: &BTreeSet<&String>) -> Option<&'a String> {
    variables(atom).find(|name| !bound.contains(*name))
}

pub(crate) fn validate_body<C>(
    body: &[Literal<C>],
    bound: &BTreeSet<&String>,
    arities: &mut DeclaredArities<'_>,
    location: impl Fn(usize) -> Location,
) -> Result<(), Error> {
    for (index, literal) in body.iter().enumerate() {
        let atom = atom(literal);
        arities.declare(&atom.relation, atom.terms.len(), location(index))?;
        if matches!(literal, Literal::Negative(_))
            && let Some(name) = unbound_variable(atom, bound)
        {
            return Err(Error {
                location: location(index),
                kind: ErrorKind::UnboundVariable(name.clone()),
            });
        }
    }
    Ok(())
}

/// Depth-first finish order along edges from body relations to rule heads.
fn dependent_postorder(dependents: &[Vec<usize>]) -> Vec<usize> {
    let mut order = Vec::with_capacity(dependents.len());
    let mut visited = vec![false; dependents.len()];
    let mut pending = Vec::new();
    for root in 0..dependents.len() {
        if std::mem::replace(&mut visited[root], true) {
            continue;
        }
        pending.push((root, dependents[root].iter()));
        while let Some((relation, next)) = pending.last_mut() {
            if let Some(&dependent) = next.next() {
                if !std::mem::replace(&mut visited[dependent], true) {
                    pending.push((dependent, dependents[dependent].iter()));
                }
            } else {
                order.push(*relation);
                pending.pop();
            }
        }
    }
    order
}

/// The stratum of every declared relation name.
///
/// A negative edge inside a strongly connected component forms a cycle. Otherwise, each
/// component takes the highest stratum of its dependencies, adding one for a negative edge.
fn stratify<C>(
    rules: &[Rule<C>],
    arities: &BTreeMap<String, usize>,
) -> Result<BTreeMap<String, usize>, Error> {
    if rules.is_empty() {
        return Ok(arities.keys().cloned().map(|name| (name, 0)).collect());
    }

    let ids: HashMap<_, _> = arities
        .keys()
        .map(String::as_str)
        .zip(0..arities.len())
        .collect();
    let mut dependents = vec![Vec::new(); ids.len()];
    let mut dependencies = vec![Vec::new(); ids.len()];
    for (rule_index, rule) in rules.iter().enumerate() {
        let head = ids[rule.head.relation.name.as_str()];
        for (atom_index, literal) in rule.body.iter().enumerate() {
            let body = ids[atom(literal).relation.name.as_str()];
            let negated_at =
                matches!(literal, Literal::Negative(_)).then_some((rule_index, atom_index));
            dependents[body].push(head);
            dependencies[head].push((body, negated_at));
        }
    }

    let mut components = vec![None; ids.len()];
    let mut strata = vec![0; ids.len()];
    let mut first_cycle = None;
    let mut pending = Vec::new();
    // Traversing the reverse finish order along head-to-body edges visits each component
    // after its dependencies. Any unassigned relation reached here belongs to this component.
    for root in dependent_postorder(&dependents).into_iter().rev() {
        if components[root].is_some() {
            continue;
        }
        components[root] = Some(root);
        pending.push(root);
        while let Some(head) = pending.pop() {
            for &(body, negated_at) in &dependencies[head] {
                let component = *components[body].get_or_insert_with(|| {
                    pending.push(body);
                    root
                });
                if component != root {
                    let required = strata[component] + usize::from(negated_at.is_some());
                    strata[root] = strata[root].max(required);
                } else if let Some(location) = negated_at
                    && first_cycle.is_none_or(|first| location < first)
                {
                    first_cycle = Some(location);
                }
            }
        }
    }
    if let Some((rule, atom)) = first_cycle {
        return Err(Error {
            location: Location::RuleBody { rule, atom },
            kind: ErrorKind::NegativeCycle,
        });
    }
    Ok(arities
        .keys()
        .cloned()
        .zip(
            components
                .into_iter()
                .map(|component| strata[component.unwrap()]),
        )
        .collect())
}

/// Collect rows into their relation buffers and validate them in caller order.
///
/// A buffer is only read after every row passes validation. Each source stays alive while its
/// values are pushed.
fn collect_facts<C, I, R, T>(
    rows: I,
    arities: &mut DeclaredArities<'_>,
) -> Result<BTreeMap<String, RowBuffer<C>>, Error>
where
    I: IntoIterator<Item = (R, T)>,
    R: Borrow<Relation>,
    T: IntoIterator<Item = C>,
{
    let mut groups = BTreeMap::<String, RowBuffer<C>>::new();
    for (index, (source, values)) in rows.into_iter().enumerate() {
        let relation = source.borrow();
        let actual = if let Some(buffer) = groups.get_mut(&relation.name) {
            buffer.push_row(values)
        } else {
            let mut buffer = RowBuffer::new(relation.arity);
            let actual = buffer.push_row(values);
            groups.insert(relation.name.clone(), buffer);
            actual
        };
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
        let bound = bound_variables(&rule.body);
        validate_body(&rule.body, &bound, arities, |atom| Location::RuleBody {
            rule: index,
            atom,
        })?;
        if let Some(name) = unbound_variable(&rule.head, &bound) {
            return Err(Error {
                location: Location::RuleHead(index),
                kind: ErrorKind::UnboundVariable(name.clone()),
            });
        }
    }
    Ok(())
}

impl<C: Ord> Database<C> {
    /// Validate ground tuples and rules and build an immutable snapshot.
    ///
    /// Tuples and then rules are validated in caller order. Within a rule, the head's relation,
    /// the body atoms, then the head's variable bindings are checked in that order.
    /// Negative dependencies must be stratified, or the clause closing a cycle is reported as
    /// `NegativeCycle` at its negative atom.
    pub fn new(facts: Vec<(Relation, Vec<C>)>, rules: Vec<Rule<C>>) -> Result<Self, Error> {
        Self::from_rows(facts, rules)
    }

    /// Construct a snapshot from rows whose relation may be borrowed.
    ///
    /// Each row source outlives the values it yields. Errors are located as in [`Database::new`].
    pub fn from_rows<I, R, T>(rows: I, mut rules: Vec<Rule<C>>) -> Result<Self, Error>
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
            .map(|(name, buffer)| (name, buffer.canonicalize()))
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
