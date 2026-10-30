use crate::{
    Atom, Database, Error, ErrorKind, Literal, Location, Relation, RelationIndex, Rule, Snapshot,
    Term,
};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;
use std::sync::Arc;

pub(crate) fn atom<C>(literal: &Literal<C>) -> &Atom<C> {
    match literal {
        Literal::Positive(atom) | Literal::Negative(atom) => atom,
    }
}
fn relation_arity(
    arities: &mut BTreeMap<String, usize>,
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
        return Err(Error {
            location,
            kind: ErrorKind::Arity {
                expected: relation.arity,
                actual,
            },
        });
    }
    if let Some(&expected) = arities.get(&relation.name) {
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
    } else {
        arities.insert(relation.name.clone(), relation.arity);
    }
    Ok(())
}
pub(crate) fn variables<C>(atom: &Atom<C>) -> impl Iterator<Item = &String> {
    atom.terms.iter().filter_map(|term| match term {
        Term::Variable(name) => Some(name),
        Term::Constant(_) => None,
    })
}
pub(crate) fn validate_body<C>(
    body: &[Literal<C>],
    arities: &mut BTreeMap<String, usize>,
    location: impl Fn(usize) -> Location,
) -> Result<BTreeSet<String>, Error> {
    let mut bound = BTreeSet::new();
    for (index, literal) in body.iter().enumerate() {
        let atom = atom(literal);
        relation_arity(arities, &atom.relation, atom.terms.len(), location(index))?;
        if matches!(literal, Literal::Positive(_)) {
            bound.extend(variables(atom).cloned());
        }
    }
    for (index, literal) in body.iter().enumerate() {
        if let Literal::Negative(atom) = literal {
            for name in variables(atom) {
                if !bound.contains(name) {
                    return Err(Error {
                        location: location(index),
                        kind: ErrorKind::UnboundVariable(name.clone()),
                    });
                }
            }
        }
    }
    Ok(bound)
}
impl<C: Clone + Ord + Hash> Database<C> {
    /// Validate ground tuples and rules and construct an immutable snapshot.
    ///
    /// Relation names must be nonempty and have one arity throughout the
    /// snapshot. Negative dependencies must be stratified. Duplicate tuples
    /// and rules do not change the snapshot.
    pub fn new(mut facts: Vec<(Relation, Vec<C>)>, mut rules: Vec<Rule<C>>) -> Result<Self, Error> {
        let mut arities = BTreeMap::new();
        for (index, (relation, tuple)) in facts.iter().enumerate() {
            relation_arity(&mut arities, relation, tuple.len(), Location::Fact(index))?;
        }
        for (index, rule) in rules.iter().enumerate() {
            relation_arity(
                &mut arities,
                &rule.head.relation,
                rule.head.terms.len(),
                Location::RuleHead(index),
            )?;
            let bound = validate_body(&rule.body, &mut arities, |atom| Location::RuleBody {
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
        let mut strata: BTreeMap<String, usize> =
            arities.keys().map(|name| (name.clone(), 0)).collect();
        for pass in 0..=arities.len() {
            let mut changed = false;
            for (index, rule) in rules.iter().enumerate() {
                for (body_index, literal) in rule.body.iter().enumerate() {
                    let required = strata[&atom(literal).relation.name]
                        + usize::from(matches!(literal, Literal::Negative(_)));
                    let head = strata.get_mut(&rule.head.relation.name).unwrap();
                    if *head < required {
                        if pass == arities.len() {
                            return Err(Error {
                                location: Location::RuleBody {
                                    rule: index,
                                    atom: body_index,
                                },
                                kind: ErrorKind::NegativeCycle,
                            });
                        }
                        *head = required;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        facts.sort();
        facts.dedup();
        rules.sort();
        rules.dedup();
        let mut relations: BTreeMap<String, RelationIndex<C>> = BTreeMap::new();
        for (row, (relation, tuple)) in facts.iter().enumerate() {
            let index = relations
                .entry(relation.name.clone())
                .or_insert_with(|| RelationIndex {
                    rows: Vec::new(),
                    columns: (0..relation.arity).map(|_| BTreeMap::new()).collect(),
                });
            index.rows.push(row);
            for (column, value) in tuple.iter().enumerate() {
                index.columns[column]
                    .entry(value.clone())
                    .or_default()
                    .push(row);
            }
        }
        let mut rule_heads: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, rule) in rules.iter().enumerate() {
            rule_heads
                .entry(rule.head.relation.name.clone())
                .or_default()
                .push(index);
        }
        Ok(Self {
            snapshot: Arc::new(Snapshot {
                facts,
                rules,
                arities,
                strata,
                relations,
                rule_heads,
            }),
        })
    }
}
