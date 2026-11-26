//! Relation, arity, variable binding, and stratification validation.

use crate::relation::RowBuffer;
use crate::{Atom, Database, Error, ErrorKind, Literal, Location, Relation, Rule, Snapshot, Term};
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub(crate) fn atom<C>(lit: &Literal<C>) -> &Atom<C> {
    match lit {
        Literal::Positive(a) | Literal::Negative(a) => a,
    }
}

pub(crate) fn variables<C>(atom: &Atom<C>) -> impl Iterator<Item = &String> {
    atom.terms.iter().filter_map(|t| match t {
        Term::Variable(v) => Some(v),
        Term::Constant(_) => None,
    })
}

fn arity_error(location: Location, expected: usize, actual: usize) -> Error {
    Error {
        location,
        kind: ErrorKind::Arity { expected, actual },
    }
}

fn declare_relations(
    rels: impl IntoIterator<Item = impl Borrow<Relation>>,
) -> Result<BTreeMap<String, usize>, Error> {
    let mut arities = BTreeMap::new();
    for (idx, rel) in rels.into_iter().enumerate() {
        let rel = rel.borrow();
        let location = Location::Relation(idx);
        if rel.name.is_empty() {
            return Err(Error {
                location,
                kind: ErrorKind::EmptyRelation,
            });
        }
        if arities.insert(rel.name.clone(), rel.arity).is_some() {
            return Err(Error {
                location,
                kind: ErrorKind::DuplicateRelation(rel.name.clone()),
            });
        }
    }
    Ok(arities)
}

fn validate_relation(
    arities: &BTreeMap<String, usize>,
    rel: &Relation,
    actual: usize,
    location: Location,
) -> Result<(), Error> {
    if rel.arity != actual {
        return Err(arity_error(location, rel.arity, actual));
    }
    let Some(&expected) = arities.get(&rel.name) else {
        return Err(Error {
            location,
            kind: ErrorKind::UnknownRelation(rel.name.clone()),
        });
    };
    if expected != rel.arity {
        return Err(Error {
            location,
            kind: ErrorKind::RelationArity {
                name: rel.name.clone(),
                expected,
                actual: rel.arity,
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

pub(crate) fn bound_variables<C>(body: &[Literal<C>]) -> BTreeSet<&String> {
    body.iter()
        .filter_map(|lit| match lit {
            Literal::Positive(a) => Some(a),
            Literal::Negative(_) => None,
        })
        .flat_map(variables)
        .collect()
}

fn unbound_variable<'a, C>(atom: &'a Atom<C>, bound: &BTreeSet<&String>) -> Option<&'a String> {
    variables(atom).find(|v| !bound.contains(*v))
}

pub(crate) fn validate_body<C>(
    body: &[Literal<C>],
    bound: &BTreeSet<&String>,
    arities: &BTreeMap<String, usize>,
    loc: impl Fn(usize) -> Location,
) -> Result<(), Error> {
    for (idx, lit) in body.iter().enumerate() {
        let a = atom(lit);
        validate_relation(arities, &a.relation, a.terms.len(), loc(idx))?;
        validate_variables(a, loc(idx))?;
        if matches!(lit, Literal::Negative(_))
            && let Some(var) = unbound_variable(a, bound)
        {
            return Err(Error {
                location: loc(idx),
                kind: ErrorKind::UnboundVariable(var.clone()),
            });
        }
    }
    Ok(())
}

fn dependent_postorder(dependents: &[Vec<usize>]) -> Vec<usize> {
    let mut order = Vec::with_capacity(dependents.len());
    let mut visited = vec![false; dependents.len()];
    let mut stack = Vec::new();
    for root in 0..dependents.len() {
        if std::mem::replace(&mut visited[root], true) {
            continue;
        }
        stack.push((root, dependents[root].iter()));
        while let Some((rel, next)) = stack.last_mut() {
            if let Some(&dep) = next.next() {
                if !std::mem::replace(&mut visited[dep], true) {
                    stack.push((dep, dependents[dep].iter()));
                }
            } else {
                order.push(*rel);
                stack.pop();
            }
        }
    }
    order
}

/// Assign each relation a stratum; any negative edge inside an SCC is a stratification cycle.
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
    let mut deps = vec![Vec::new(); ids.len()];
    for (rule_idx, rule) in rules.iter().enumerate() {
        let head = ids[rule.head.relation.name.as_str()];
        for (atom_idx, lit) in rule.body.iter().enumerate() {
            let body = ids[atom(lit).relation.name.as_str()];
            let neg_at = matches!(lit, Literal::Negative(_)).then_some((rule_idx, atom_idx));
            dependents[body].push(head);
            deps[head].push((body, neg_at));
        }
    }

    let mut sccs = vec![None; ids.len()];
    let mut strata = vec![0; ids.len()];
    let mut first_cycle = None;
    let mut stack = Vec::new();
    // Kosaraju reverse-postorder on head->body edges visits dependencies before dependents.
    for root in dependent_postorder(&dependents).into_iter().rev() {
        if sccs[root].is_some() {
            continue;
        }
        sccs[root] = Some(root);
        stack.push(root);
        while let Some(head) = stack.pop() {
            for &(body, neg_at) in &deps[head] {
                let scc = *sccs[body].get_or_insert_with(|| {
                    stack.push(body);
                    root
                });
                if scc != root {
                    let req = strata[scc] + usize::from(neg_at.is_some());
                    strata[root] = strata[root].max(req);
                } else if let Some(loc) = neg_at
                    && first_cycle.is_none_or(|first| loc < first)
                {
                    first_cycle = Some(loc);
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
        .zip(sccs.into_iter().map(|scc| strata[scc.unwrap()]))
        .collect())
}

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
    for (idx, (src, vals)) in rows.into_iter().enumerate() {
        let rel = src.borrow();
        let actual = if let Some(buf) = groups.get_mut(&rel.name) {
            buf.push_row(vals)
        } else {
            let mut buf = RowBuffer::new(rel.arity);
            let actual = buf.push_row(vals);
            groups.insert(rel.name.clone(), buf);
            actual
        };
        validate_relation(arities, rel, actual, Location::Fact(idx))?;
    }
    Ok(groups)
}

fn validate_rules<C>(rules: &[Rule<C>], arities: &BTreeMap<String, usize>) -> Result<(), Error> {
    for (idx, rule) in rules.iter().enumerate() {
        let head = Location::RuleHead(idx);
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
            rule: idx,
            atom,
        })?;
        if let Some(var) = unbound_variable(&rule.head, &bound) {
            return Err(Error {
                location: Location::RuleHead(idx),
                kind: ErrorKind::UnboundVariable(var.clone()),
            });
        }
    }
    Ok(())
}

impl<C: Ord> Database<C> {
    /// Validate relation declarations, ground facts, and rules, returning an immutable snapshot.
    pub fn new(
        relations: Vec<Relation>,
        facts: Vec<(Relation, Vec<C>)>,
        rules: Vec<Rule<C>>,
    ) -> Result<Self, Error> {
        Self::from_rows(relations, facts, rules)
    }

    /// Build a snapshot from declarations, streamed fact rows, and rules in caller order.
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

        // Canonicalize only after all validation succeeds.
        let relations = groups
            .into_iter()
            .map(|(name, buf)| (name, buf.canonicalize()))
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
        for (idx, compiled) in rules.iter().enumerate() {
            rules_by_head
                .entry(compiled.rule.head.relation.name.clone())
                .or_default()
                .push(idx);
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
