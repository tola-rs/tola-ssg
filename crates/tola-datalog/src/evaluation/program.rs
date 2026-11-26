//! Compilation of rule and query bodies into value-independent stage plans.

use crate::validation::atom;
use crate::{Literal, Query, Rule, Term};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) enum Argument {
    Slot(usize),
    Constant(usize),
}

struct BodyAtom {
    literal: usize,
    arguments: Vec<Argument>,
    negative: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Column {
    Input(usize),
    Tuple(usize),
    ParameterOrTuple { slot: usize, column: usize },
    HeadConstant(usize),
}

pub(super) enum Constraint {
    Input(usize),
    Parameter(usize),
    Repeated {
        first: usize,
        parameter: Option<usize>,
    },
    Constant,
    Bind,
}

pub(crate) struct Program {
    pub(super) stages: Vec<Stage>,
    pub(super) head: Vec<Argument>,
    pub(super) slots: usize,
}

pub(super) struct Stage {
    pub(super) literal: usize,
    pub(super) negative: bool,
    pub(super) arguments: Vec<Constraint>,
    pub(super) output: Vec<Column>,
    pub(super) preserves_order: bool,
}

pub(crate) struct RuleProgram<C> {
    pub(crate) rule: Rule<C>,
    pub(crate) program: Program,
}

impl<C: std::fmt::Debug> std::fmt::Debug for RuleProgram<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.rule.fmt(f)
    }
}

impl Program {
    fn arguments<'a, C>(
        terms: &'a [Term<C>],
        slots: &mut BTreeMap<&'a str, usize>,
    ) -> Vec<Argument> {
        terms
            .iter()
            .enumerate()
            .map(|(idx, term)| match term {
                Term::Constant(_) => Argument::Constant(idx),
                Term::Variable(var) => {
                    let next = slots.len();
                    Argument::Slot(*slots.entry(var.as_str()).or_insert(next))
                }
            })
            .collect()
    }

    /// Order positive atoms before negative atoms while preserving relative body order.
    fn atoms<'a, C>(body: &'a [Literal<C>], slots: &mut BTreeMap<&'a str, usize>) -> Vec<BodyAtom> {
        body.iter()
            .enumerate()
            .filter(|(_, lit)| matches!(lit, Literal::Positive(_)))
            .chain(
                body.iter()
                    .enumerate()
                    .filter(|(_, lit)| matches!(lit, Literal::Negative(_))),
            )
            .map(|(literal, lit)| BodyAtom {
                literal,
                arguments: Self::arguments(&atom(lit).terms, slots),
                negative: matches!(lit, Literal::Negative(_)),
            })
            .collect()
    }

    pub(crate) fn rule<C>(rule: &Rule<C>) -> Self {
        let mut slots = BTreeMap::new();
        let atoms = Self::atoms(&rule.body, &mut slots);
        let head = Self::arguments(&rule.head.terms, &mut slots);
        let params = head
            .iter()
            .filter_map(|arg| match arg {
                Argument::Slot(slot) => Some(*slot),
                Argument::Constant(_) => None,
            })
            .collect();
        Self::compile(atoms, head, slots.len(), params)
    }

    pub(super) fn query<C>(query: &Query<C>) -> Self {
        let mut slots = BTreeMap::new();
        let atoms = Self::atoms(&query.body, &mut slots);
        let head = query
            .select
            .iter()
            .map(|var| Argument::Slot(slots[var.as_str()]))
            .collect();
        Self::compile(atoms, head, slots.len(), BTreeSet::new())
    }

    fn compile(
        atoms: Vec<BodyAtom>,
        head: Vec<Argument>,
        slots: usize,
        params: BTreeSet<usize>,
    ) -> Self {
        let mut liveness = Liveness::new(&atoms, &head, slots);
        let mut stages = Vec::with_capacity(atoms.len());
        // Walk backwards so each slot stays live until the last stage that reads it.
        for pos in (0..atoms.len()).rev() {
            let body_atom = &atoms[pos];
            let (input, output) = liveness.stage_slots(body_atom, pos);
            let head = (pos + 1 == atoms.len()).then_some(head.as_slice());
            stages.push(Stage::compile(body_atom, &input, &output, head, &params));
        }
        stages.reverse();
        Self {
            stages,
            head,
            slots,
        }
    }
}

struct Liveness {
    first_bound: Vec<usize>,
    needed: Vec<bool>,
}

impl Liveness {
    fn new(atoms: &[BodyAtom], head: &[Argument], slots: usize) -> Self {
        let mut first_bound = vec![None; slots];
        for (pos, body_atom) in atoms.iter().enumerate() {
            if body_atom.negative {
                continue;
            }
            for arg in &body_atom.arguments {
                if let Argument::Slot(slot) = arg {
                    first_bound[*slot].get_or_insert(pos + 1);
                }
            }
        }
        let mut needed = vec![false; slots];
        for arg in head {
            if let Argument::Slot(slot) = arg {
                needed[*slot] = true;
            }
        }
        Self {
            first_bound: first_bound.into_iter().map(Option::unwrap).collect(),
            needed,
        }
    }

    fn stage_slots(&mut self, body_atom: &BodyAtom, pos: usize) -> (Vec<usize>, Vec<usize>) {
        let first_bound = &self.first_bound;
        let needed = &mut self.needed;
        let stage = pos + 1;
        let output = needed
            .iter()
            .enumerate()
            .filter_map(|(slot, &n)| (n && first_bound[slot] <= stage).then_some(slot))
            .collect();
        for arg in &body_atom.arguments {
            if let Argument::Slot(slot) = arg
                && first_bound[*slot] < stage
            {
                needed[*slot] = true;
            }
        }
        // Drop slots first bound at or after `stage` so earlier stages don't carry them.
        for (slot, n) in needed.iter_mut().enumerate() {
            *n &= first_bound[slot] < stage;
        }
        let input = needed
            .iter()
            .enumerate()
            .filter_map(|(slot, &n)| n.then_some(slot))
            .collect();
        (input, output)
    }
}

impl Stage {
    fn compile(
        body_atom: &BodyAtom,
        input: &[usize],
        output_slots: &[usize],
        head: Option<&[Argument]>,
        params: &BTreeSet<usize>,
    ) -> Self {
        let mut first: Vec<(usize, usize)> = Vec::new();
        let arguments: Vec<Constraint> = body_atom
            .arguments
            .iter()
            .enumerate()
            .map(|(col, arg)| match arg {
                Argument::Constant(_) => Constraint::Constant,
                Argument::Slot(slot) => match input.binary_search(slot) {
                    Ok(idx) => Constraint::Input(idx),
                    Err(_) => match first.binary_search_by_key(slot, |(s, _)| *s) {
                        Ok(idx) => Constraint::Repeated {
                            first: first[idx].1,
                            parameter: params.contains(slot).then_some(*slot),
                        },
                        Err(idx) => {
                            first.insert(idx, (*slot, col));
                            if params.contains(slot) {
                                Constraint::Parameter(*slot)
                            } else {
                                Constraint::Bind
                            }
                        }
                    },
                },
            })
            .collect();
        let read_slot = |slot: usize| match input.binary_search(&slot) {
            Ok(idx) => Column::Input(idx),
            Err(_) => {
                let col = first[first.binary_search_by_key(&slot, |(s, _)| *s).unwrap()].1;
                if params.contains(&slot) {
                    Column::ParameterOrTuple { slot, column: col }
                } else {
                    Column::Tuple(col)
                }
            }
        };
        let output: Vec<Column> = if let Some(head) = head {
            head.iter()
                .map(|arg| match arg {
                    Argument::Slot(slot) => read_slot(*slot),
                    Argument::Constant(col) => Column::HeadConstant(*col),
                })
                .collect()
        } else {
            output_slots.iter().copied().map(read_slot).collect()
        };
        let preserves_order = preserves_order(&arguments, &output);
        Self {
            literal: body_atom.literal,
            negative: body_atom.negative,
            arguments,
            output,
            preserves_order,
        }
    }
}

fn preserves_order(arguments: &[Constraint], output: &[Column]) -> bool {
    let introduced = arguments.iter().enumerate().filter_map(|(col, c)| {
        matches!(c, Constraint::Parameter(_) | Constraint::Bind).then_some(col)
    });
    let mut emitted = Vec::new();
    for col in output {
        if let Column::Tuple(idx) | Column::ParameterOrTuple { column: idx, .. } = col
            && !emitted.contains(idx)
        {
            emitted.push(*idx);
        }
    }
    introduced.eq(emitted)
}
