//! Relation declarations, arity, variable binding, and stratification checks.
//!
//! Reporting follows the caller's order: declarations, facts, then rules. Within a clause, atoms
//! retain their written order.
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

fn declare_relations(
    relations: impl IntoIterator<Item = impl Borrow<Relation>>,
) -> Result<BTreeMap<String, usize>, Error> {
    let mut arities = BTreeMap::new();
    for (index, relation) in relations.into_iter().enumerate() {
        let relation = relation.borrow();
        let location = Location::Relation(index);
        if relation.name.is_empty() {
            return Err(Error {
                location,
                kind: ErrorKind::EmptyRelation,
            });
        }
        if arities
            .insert(relation.name.clone(), relation.arity)
            .is_some()
        {
            return Err(Error {
                location,
                kind: ErrorKind::DuplicateRelation(relation.name.clone()),
            });
        }
    }
    Ok(arities)
}

fn validate_relation(
    arities: &BTreeMap<String, usize>,
    relation: &Relation,
    actual: usize,
    location: Location,
) -> Result<(), Error> {
    if relation.arity != actual {
        return Err(arity_error(location, relation.arity, actual));
    }
    let Some(&expected) = arities.get(&relation.name) else {
        return Err(Error {
            location,
            kind: ErrorKind::UnknownRelation(relation.name.clone()),
        });
    };
    if expected != relation.arity {
        return Err(Error {
            location,
            kind: ErrorKind::RelationArity {
                name: relation.name.clone(),
                expected,
                actual: relation.arity,
            },
        });
    }
    Ok(())
}

fn validate_variables<C>(atom: &Atom<C>, location: Location) -> Result<(), Error> {
    if variables(atom).any(String::is_empty) {
        return Err(Error {
            location,
            kind: ErrorKind::EmptyVariable,
        });
    }
    Ok(())
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
    arities: &BTreeMap<String, usize>,
    location: impl Fn(usize) -> Location,
) -> Result<(), Error> {
    for (index, literal) in body.iter().enumerate() {
        let atom = atom(literal);
        validate_relation(arities, &atom.relation, atom.terms.len(), location(index))?;
        validate_variables(atom, location(index))?;
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
    arities: &BTreeMap<String, usize>,
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
        validate_relation(arities, relation, actual, Location::Fact(index))?;
    }
    Ok(groups)
}

fn validate_rules<C>(rules: &[Rule<C>], arities: &BTreeMap<String, usize>) -> Result<(), Error> {
    for (index, rule) in rules.iter().enumerate() {
        let head = Location::RuleHead(index);
        validate_relation(
            arities,
            &rule.head.relation,
            rule.head.terms.len(),
            head.clone(),
        )?;
        if rule.body.is_empty() {
            return Err(Error {
                location: head,
                kind: ErrorKind::EmptyBody,
            });
        }
        validate_variables(&rule.head, head)?;
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
    /// Validate relation declarations, ground tuples, and rules and build a snapshot.
    pub fn new(
        relations: Vec<Relation>,
        facts: Vec<(Relation, Vec<C>)>,
        rules: Vec<Rule<C>>,
    ) -> Result<Self, Error> {
        Self::from_rows(relations, facts, rules)
    }

    /// Build a snapshot from declarations and rows whose relations may be borrowed.
    ///
    /// Declarations, facts, and rules are validated in that order. Each row source
    /// stays alive while its values are read.
    pub fn from_rows<I, R, T>(
        relations: impl IntoIterator<Item = impl Borrow<Relation>>,
        rows: I,
        mut rules: Vec<Rule<C>>,
    ) -> Result<Self, Error>
    where
        I: IntoIterator<Item = (R, T)>,
        R: Borrow<Relation>,
        T: IntoIterator<Item = C>,
    {
        let arities = declare_relations(relations)?;
        let groups = collect_facts(rows, &arities)?;
        validate_rules(&rules, &arities)?;
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
