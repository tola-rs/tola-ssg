//! Compilation of a rule or query body into value-independent stages.
//!
//! A program holds slot and column positions, never constants or rows: a rule's program is compiled
//! once with the snapshot and reused, while a query's is compiled per call. Neither may depend on
//! the values being joined.
use crate::validation::atom;
use crate::{Literal, Query, Rule, Term};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) enum Argument {
    Slot(usize),
    Constant(usize),
}

/// One body atom compiled to slot numbers, tagged with its position in `Rule::body`.
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

/// A rule and the program compiled from it, kept together so they cannot drift apart.
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
            .map(|(index, term)| match term {
                Term::Constant(_) => Argument::Constant(index),
                Term::Variable(name) => {
                    let next = slots.len();
                    Argument::Slot(*slots.entry(name.as_str()).or_insert(next))
                }
            })
            .collect()
    }

    /// Positive atoms first, then negative atoms; both groups keep their body order.
    fn atoms<'a, C>(body: &'a [Literal<C>], slots: &mut BTreeMap<&'a str, usize>) -> Vec<BodyAtom> {
        body.iter()
            .enumerate()
            .filter(|(_, literal)| matches!(literal, Literal::Positive(_)))
            .chain(
                body.iter()
                    .enumerate()
                    .filter(|(_, literal)| matches!(literal, Literal::Negative(_))),
            )
            .map(|(literal, body_atom)| BodyAtom {
                literal,
                arguments: Self::arguments(&atom(body_atom).terms, slots),
                negative: matches!(body_atom, Literal::Negative(_)),
            })
            .collect()
    }

    pub(crate) fn rule<C>(rule: &Rule<C>) -> Self {
        let mut slots = BTreeMap::new();
        let atoms = Self::atoms(&rule.body, &mut slots);
        let head = Self::arguments(&rule.head.terms, &mut slots);
        let parameters = head
            .iter()
            .filter_map(|argument| match argument {
                Argument::Slot(slot) => Some(*slot),
                Argument::Constant(_) => None,
            })
            .collect();
        Self::compile(atoms, head, slots.len(), parameters)
    }

    pub(super) fn query<C>(query: &Query<C>) -> Self {
        let mut slots = BTreeMap::new();
        let atoms = Self::atoms(&query.body, &mut slots);
        let head = query
            .select
            .iter()
            .map(|name| Argument::Slot(slots[name.as_str()]))
            .collect();
        Self::compile(atoms, head, slots.len(), BTreeSet::new())
    }

    /// Assign one stage per atom, walking the atoms backwards so every slot stays available until
    /// the last stage that reads it.
    fn compile(
        atoms: Vec<BodyAtom>,
        head: Vec<Argument>,
        slots: usize,
        parameters: BTreeSet<usize>,
    ) -> Self {
        let mut liveness = Liveness::new(&atoms, &head, slots);
        let mut stages = Vec::with_capacity(atoms.len());
        for position in (0..atoms.len()).rev() {
            let body_atom = &atoms[position];
            let (input, output) = liveness.stage_slots(body_atom, position);
            // The last stage emits the head projection instead of its own read slots.
            let head = (position + 1 == atoms.len()).then_some(head.as_slice());
            stages.push(Stage::compile(
                body_atom,
                &input,
                &output,
                head,
                &parameters,
            ));
        }
        stages.reverse();
        Self {
            stages,
            head,
            slots,
        }
    }
}

/// The first stage that binds each slot, and the slots a stage to the left must keep live.
/// `Program::compile` walks the atoms backwards, one stage at a time.
struct Liveness {
    /// 1-based position of the first positive atom binding each slot; validation guarantees one.
    first_bound: Vec<usize>,
    /// Slots that a stage still to be compiled, or the head, reads.
    needed: Vec<bool>,
}
impl Liveness {
    fn new(atoms: &[BodyAtom], head: &[Argument], slots: usize) -> Self {
        let mut first_bound = vec![None; slots];
        for (position, body_atom) in atoms.iter().enumerate() {
            if body_atom.negative {
                continue;
            }
            for argument in &body_atom.arguments {
                if let Argument::Slot(slot) = argument {
                    first_bound[*slot].get_or_insert(position + 1);
                }
            }
        }
        let mut needed = vec![false; slots];
        for argument in head {
            if let Argument::Slot(slot) = argument {
                needed[*slot] = true;
            }
        }
        Self {
            first_bound: first_bound.into_iter().map(Option::unwrap).collect(),
            needed,
        }
    }

    /// Slots the stage at `position` reads from the stages before it and writes for the ones after
    /// it, both in slot order. Afterwards `needed` describes the stages before it, so the next
    /// call must be the stage to its left.
    fn stage_slots(&mut self, body_atom: &BodyAtom, position: usize) -> (Vec<usize>, Vec<usize>) {
        let first_bound = &self.first_bound;
        let needed = &mut self.needed;
        // The stage index in `first_bound` terms.
        let stage = position + 1;
        let output = needed
            .iter()
            .enumerate()
            .filter_map(|(slot, &needed)| (needed && first_bound[slot] <= stage).then_some(slot))
            .collect();
        for argument in &body_atom.arguments {
            if let Argument::Slot(slot) = argument
                && first_bound[*slot] < stage
            {
                needed[*slot] = true;
            }
        }
        // A slot first bound here or later cannot be an input of this stage or of any stage to its
        // left, so it leaves `needed` for both.
        for (slot, needed) in needed.iter_mut().enumerate() {
            *needed &= first_bound[slot] < stage;
        }
        let input = needed
            .iter()
            .enumerate()
            .filter_map(|(slot, &needed)| needed.then_some(slot))
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
        parameters: &BTreeSet<usize>,
    ) -> Self {
        // Each slot's first occurrence in this atom; later occurrences read that column.
        let mut first: Vec<(usize, usize)> = Vec::new();
        let arguments: Vec<Constraint> = body_atom
            .arguments
            .iter()
            .enumerate()
            .map(|(column, argument)| match argument {
                Argument::Constant(_) => Constraint::Constant,
                Argument::Slot(slot) => match input.binary_search(slot) {
                    Ok(index) => Constraint::Input(index),
                    Err(_) => match first.binary_search_by_key(slot, |(slot, _)| *slot) {
                        Ok(index) => Constraint::Repeated {
                            first: first[index].1,
                            parameter: parameters.contains(slot).then_some(*slot),
                        },
                        Err(index) => {
                            first.insert(index, (*slot, column));
                            if parameters.contains(slot) {
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
            Ok(index) => Column::Input(index),
            Err(_) => {
                let column = first[first
                    .binary_search_by_key(&slot, |(slot, _)| *slot)
                    .unwrap()]
                .1;
                if parameters.contains(&slot) {
                    Column::ParameterOrTuple { slot, column }
                } else {
                    Column::Tuple(column)
                }
            }
        };
        let output: Vec<Column> = if let Some(head) = head {
            head.iter()
                .map(|argument| match argument {
                    Argument::Slot(slot) => read_slot(*slot),
                    Argument::Constant(column) => Column::HeadConstant(*column),
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

// Fixed and repeated tuple columns cannot distinguish rows before a newly bound value.
fn preserves_order(arguments: &[Constraint], output: &[Column]) -> bool {
    let introduced = arguments
        .iter()
        .enumerate()
        .filter_map(|(column, constraint)| {
            matches!(constraint, Constraint::Parameter(_) | Constraint::Bind).then_some(column)
        });
    let mut emitted = Vec::new();
    for column in output {
        if let Column::Tuple(index) | Column::ParameterOrTuple { column: index, .. } = column
            && !emitted.contains(index)
        {
            emitted.push(*index);
        }
    }
    introduced.eq(emitted)
}
