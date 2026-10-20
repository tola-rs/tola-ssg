//! The clause one compiled program is evaluated for, kept beside the rule or query it came from.
//!
//! The two must be built from the same clause: a program's stage and literal indexes mean nothing
//! against another clause's body.
use super::ConstantRef;
use super::program::{Argument, Column, Constraint, Program, RuleProgram, Stage};
use crate::validation::atom;
use crate::{Literal, Query, Term};

/// A program together with the rule or query it was compiled from: the program stores positions,
/// this type lends them the body literals and head terms, so constants stay borrowed for the whole
/// evaluation.
pub(super) struct Clause<'a, C> {
    program: &'a Program,
    body: &'a [Literal<C>],
    head: &'a [Term<C>],
}

impl<'a, C> Clause<'a, C> {
    pub(super) fn rule(compiled: &'a RuleProgram<C>) -> Self {
        Self {
            program: &compiled.program,
            body: &compiled.rule.body,
            head: &compiled.rule.head.terms,
        }
    }
    /// The clause of one query; queries project variables and have no head.
    pub(super) fn query(query: &'a Query<C>, program: &'a Program) -> Self {
        Self {
            program,
            body: &query.body,
            head: &[],
        }
    }
    /// Number of body atoms, one stage each.
    pub(super) fn len(&self) -> usize {
        self.program.stages.len()
    }
    /// The stage compiled for the body atom at `position`.
    pub(super) fn stage(&self, position: usize) -> &Stage {
        &self.program.stages[position]
    }
    pub(super) fn relation(&self, stage: &Stage) -> &'a str {
        &atom(&self.body[stage.literal]).relation.name
    }
    fn constant(&self, stage: &Stage, column: usize) -> &'a C {
        match &atom(&self.body[stage.literal]).terms[column] {
            Term::Constant(value) => value,
            _ => unreachable!("the program marks a column constant only where the atom writes one"),
        }
    }
    fn head_constant(&self, column: usize) -> &'a C {
        match &self.head[column] {
            Term::Constant(value) => value,
            _ => unreachable!("the program records a head constant only where the term is one"),
        }
    }
    /// The one row a clause starts from: its head constants when it has no body, otherwise an
    /// empty binding.
    pub(super) fn initial(&self) -> Vec<ConstantRef<'a, C>> {
        if self.len() == 0 {
            self.program
                .head
                .iter()
                .map(|term| match term {
                    Argument::Constant(column) => ConstantRef(self.head_constant(*column)),
                    Argument::Slot(_) => {
                        unreachable!(
                            "a body-less rule binds no slot, and validation rejects a head variable"
                        )
                    }
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}
impl<'a, C: Ord> Clause<'a, C> {
    /// Turns the demand's bound head columns into per-slot parameters.
    ///
    /// `None` means no answer of this rule can satisfy the demand: it pins a head constant to a
    /// different value, or pins one slot to two different values.
    pub(super) fn parameters(
        &self,
        values: impl IntoIterator<Item = Option<ConstantRef<'a, C>>>,
    ) -> Option<Vec<Option<ConstantRef<'a, C>>>> {
        let mut parameters = vec![None; self.program.slots];
        for (argument, value) in self.program.head.iter().zip(values) {
            let Some(value) = value else { continue };
            match argument {
                Argument::Constant(column) => {
                    if ConstantRef(self.head_constant(*column)) != value {
                        return None;
                    }
                }
                Argument::Slot(slot) => match parameters[*slot] {
                    Some(bound) if bound != value => return None,
                    Some(_) => {}
                    None => parameters[*slot] = Some(value),
                },
            }
        }
        Some(parameters)
    }
}

/// Which columns of a candidate row are already known to equal the wanted value, so
/// `Stage::matches` can skip comparing them.
#[derive(Clone, Copy)]
pub(super) enum MatchEvidence {
    /// Nothing is known; every bound column is compared.
    None,
    /// The rows were selected by this column's index.
    Column(usize),
    /// Table answers satisfy every bound column of their demand.
    Demand,
}

impl MatchEvidence {
    fn covers<C>(
        self,
        column: usize,
        constraint: &Constraint,
        parameters: &[Option<ConstantRef<'_, C>>],
    ) -> bool {
        match self {
            Self::None => false,
            Self::Column(proven) => proven == column,
            Self::Demand => match constraint {
                Constraint::Repeated { parameter, .. } => {
                    parameter.is_some_and(|slot| parameters[slot].is_some())
                }
                _ => true,
            },
        }
    }
}

impl Stage {
    /// Number of columns of the body atom.
    pub(super) fn arity(&self) -> usize {
        self.arguments.len()
    }
    /// Number of values in one output row.
    pub(super) fn width(&self) -> usize {
        self.output.len()
    }

    /// Whether the constraint supplies the column's value from the clause, not from the candidate
    /// tuple.
    pub(super) fn is_fixed_column(&self, column: usize) -> bool {
        matches!(
            self.arguments[column],
            Constraint::Constant
                | Constraint::Parameter(_)
                | Constraint::Repeated {
                    parameter: Some(_),
                    ..
                }
        )
    }
    /// The input slot this column repeats, when an earlier stage bound it.
    pub(super) fn input_column(&self, column: usize) -> Option<usize> {
        match self.arguments[column] {
            Constraint::Input(index) => Some(index),
            _ => None,
        }
    }
    /// Whether the column is a constant of the clause.
    pub(super) fn is_constant_column(&self, column: usize) -> bool {
        matches!(self.arguments[column], Constraint::Constant)
    }
    /// The value bound to the column before the tuple is read, if the constraint does not read it.
    pub(super) fn bound<'a, C>(
        &self,
        clause: &Clause<'a, C>,
        input: &[ConstantRef<'a, C>],
        parameters: &[Option<ConstantRef<'a, C>>],
        column: usize,
    ) -> Option<ConstantRef<'a, C>> {
        match self.arguments[column] {
            Constraint::Input(index) => Some(input[index]),
            Constraint::Parameter(slot)
            | Constraint::Repeated {
                parameter: Some(slot),
                ..
            } => parameters[slot],
            Constraint::Constant => Some(ConstantRef(clause.constant(self, column))),
            _ => None,
        }
    }

    /// Whether every column matches without reading the tuple, so the scan can skip `matches`.
    pub(super) fn matches_all<C>(
        &self,
        parameters: &[Option<ConstantRef<'_, C>>],
        evidence: MatchEvidence,
    ) -> bool {
        self.arguments
            .iter()
            .enumerate()
            .all(|(column, constraint)| {
                evidence.covers(column, constraint, parameters)
                    || match constraint {
                        Constraint::Bind => true,
                        Constraint::Parameter(slot) => parameters[*slot].is_none(),
                        _ => false,
                    }
            })
    }
    /// Whether the candidate tuple satisfies every constraint the evidence does not cover.
    pub(super) fn matches<'a, C: Ord>(
        &self,
        clause: &Clause<'a, C>,
        input: &[ConstantRef<'a, C>],
        parameters: &[Option<ConstantRef<'a, C>>],
        evidence: MatchEvidence,
        tuple: &impl Fn(usize) -> ConstantRef<'a, C>,
    ) -> bool {
        self.arguments
            .iter()
            .enumerate()
            .all(|(column, constraint)| {
                if evidence.covers(column, constraint, parameters) {
                    return true;
                }
                let expected = match constraint {
                    Constraint::Repeated { first, parameter } => Some(
                        parameter
                            .and_then(|slot| parameters[slot])
                            .unwrap_or_else(|| tuple(*first)),
                    ),
                    Constraint::Bind => None,
                    _ => self.bound(clause, input, parameters, column),
                };
                expected.is_none_or(|expected| expected == tuple(column))
            })
    }

    /// Whether the output row is the input row itself, so rows can be appended as they are.
    pub(super) fn identity<C>(&self, parameters: &[Option<ConstantRef<'_, C>>]) -> bool {
        self.output.len() == self.arguments.len()
            && self
                .output
                .iter()
                .enumerate()
                .all(|(column, value)| match value {
                    Column::Tuple(index) => *index == column,
                    Column::ParameterOrTuple {
                        slot,
                        column: index,
                    } => *index == column && parameters[*slot].is_none(),
                    _ => false,
                })
    }

    /// The output row for one candidate tuple, reading the input, tuple, parameters or head
    /// constants.
    pub(super) fn values<'a, 'b, C>(
        &'b self,
        clause: &'b Clause<'a, C>,
        input: &'b [ConstantRef<'a, C>],
        parameters: &'b [Option<ConstantRef<'a, C>>],
        tuple: &'b impl Fn(usize) -> ConstantRef<'a, C>,
    ) -> impl Iterator<Item = ConstantRef<'a, C>> + 'b {
        self.output.iter().map(move |column| match *column {
            Column::Input(index) => input[index],
            Column::Tuple(column) => tuple(column),
            Column::ParameterOrTuple { slot, column } => {
                parameters[slot].unwrap_or_else(|| tuple(column))
            }
            Column::HeadConstant(index) => ConstantRef(clause.head_constant(index)),
        })
    }
}
