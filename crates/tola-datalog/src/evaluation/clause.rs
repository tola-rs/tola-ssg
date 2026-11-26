//! Clause view pairing a compiled program with its source literals and head constants.

use super::ConstantRef;
use super::program::{Argument, Column, Constraint, Program, RuleProgram, Stage};
use crate::validation::atom;
use crate::{Literal, Query, Term};

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

    pub(super) fn query(query: &'a Query<C>, program: &'a Program) -> Self {
        Self {
            program,
            body: &query.body,
            head: &[],
        }
    }

    pub(super) fn len(&self) -> usize {
        self.program.stages.len()
    }

    pub(super) fn stage(&self, pos: usize) -> &Stage {
        &self.program.stages[pos]
    }

    pub(super) fn relation(&self, stage: &Stage) -> &'a str {
        &atom(&self.body[stage.literal]).relation.name
    }

    fn constant(&self, stage: &Stage, col: usize) -> &'a C {
        match &atom(&self.body[stage.literal]).terms[col] {
            Term::Constant(v) => v,
            _ => unreachable!("program marks constant only where term is constant"),
        }
    }

    fn head_constant(&self, col: usize) -> &'a C {
        match &self.head[col] {
            Term::Constant(v) => v,
            _ => unreachable!("program records head constant only where term is constant"),
        }
    }
}

impl<'a, C: Ord> Clause<'a, C> {
    /// Bind demand columns into rule parameter slots, returning `None` on mismatch.
    pub(super) fn parameters(
        &self,
        vals: impl IntoIterator<Item = Option<ConstantRef<'a, C>>>,
    ) -> Option<Vec<Option<ConstantRef<'a, C>>>> {
        let mut params = vec![None; self.program.slots];
        for (arg, val) in self.program.head.iter().zip(vals) {
            let Some(val) = val else { continue };
            match arg {
                Argument::Constant(col) => {
                    if ConstantRef(self.head_constant(*col)) != val {
                        return None;
                    }
                }
                Argument::Slot(slot) => match params[*slot] {
                    Some(bound) if bound != val => return None,
                    Some(_) => {}
                    None => params[*slot] = Some(val),
                },
            }
        }
        Some(params)
    }
}

/// Columns of a candidate row already proven equal by index or demand selection.
#[derive(Clone, Copy)]
pub(super) enum ClauseMatch {
    None,
    Column(usize),
    Prefix(usize),
    Demand,
}

impl ClauseMatch {
    pub(super) fn includes(self, col: usize) -> bool {
        match self {
            Self::Column(c) => c == col,
            Self::Prefix(end) => col < end,
            Self::None | Self::Demand => false,
        }
    }

    fn covers<C>(
        self,
        col: usize,
        constraint: &Constraint,
        params: &[Option<ConstantRef<'_, C>>],
    ) -> bool {
        match self {
            Self::Demand => match constraint {
                Constraint::Repeated { parameter, .. } => {
                    parameter.is_some_and(|slot| params[slot].is_some())
                }
                _ => true,
            },
            _ => self.includes(col),
        }
    }
}

impl Stage {
    pub(super) fn arity(&self) -> usize {
        self.arguments.len()
    }

    pub(super) fn width(&self) -> usize {
        self.output.len()
    }

    pub(super) fn input_column(&self, col: usize) -> Option<usize> {
        match self.arguments[col] {
            Constraint::Input(idx) => Some(idx),
            _ => None,
        }
    }

    pub(super) fn reads_input(&self) -> bool {
        self.arguments
            .iter()
            .any(|arg| matches!(arg, Constraint::Input(_)))
    }

    pub(super) fn is_constant_column(&self, col: usize) -> bool {
        matches!(self.arguments[col], Constraint::Constant)
    }

    pub(super) fn bound<'a, C>(
        &self,
        clause: &Clause<'a, C>,
        input: &[ConstantRef<'a, C>],
        params: &[Option<ConstantRef<'a, C>>],
        col: usize,
    ) -> Option<ConstantRef<'a, C>> {
        match self.arguments[col] {
            Constraint::Input(idx) => Some(input[idx]),
            Constraint::Parameter(slot)
            | Constraint::Repeated {
                parameter: Some(slot),
                ..
            } => params[slot],
            Constraint::Constant => Some(ConstantRef(clause.constant(self, col))),
            _ => None,
        }
    }

    pub(super) fn matches_all<C>(
        &self,
        params: &[Option<ConstantRef<'_, C>>],
        ev: ClauseMatch,
    ) -> bool {
        self.arguments.iter().enumerate().all(|(col, constraint)| {
            ev.covers(col, constraint, params)
                || match constraint {
                    Constraint::Bind => true,
                    Constraint::Parameter(slot) => params[*slot].is_none(),
                    _ => false,
                }
        })
    }

    pub(super) fn matches<'a, C: Ord>(
        &self,
        clause: &Clause<'a, C>,
        input: &[ConstantRef<'a, C>],
        params: &[Option<ConstantRef<'a, C>>],
        ev: ClauseMatch,
        tup: &impl Fn(usize) -> ConstantRef<'a, C>,
    ) -> bool {
        self.arguments.iter().enumerate().all(|(col, constraint)| {
            if ev.covers(col, constraint, params) {
                return true;
            }
            let expected = match constraint {
                Constraint::Repeated { first, parameter } => Some(
                    parameter
                        .and_then(|s| params[s])
                        .unwrap_or_else(|| tup(*first)),
                ),
                Constraint::Bind => None,
                _ => self.bound(clause, input, params, col),
            };
            expected.is_none_or(|exp| exp == tup(col))
        })
    }

    pub(super) fn identity<C>(&self, params: &[Option<ConstantRef<'_, C>>]) -> bool {
        self.output.len() == self.arguments.len()
            && self.output.iter().enumerate().all(|(col, val)| match val {
                Column::Tuple(idx) => *idx == col,
                Column::ParameterOrTuple { slot, column } => {
                    *column == col && params[*slot].is_none()
                }
                _ => false,
            })
    }

    pub(super) fn reads_tuple<C>(&self, params: &[Option<ConstantRef<'_, C>>]) -> bool {
        self.output.iter().any(|col| match *col {
            Column::Tuple(_) => true,
            Column::ParameterOrTuple { slot, .. } => params[slot].is_none(),
            Column::Input(_) | Column::HeadConstant(_) => false,
        })
    }

    pub(super) fn values<'a, 'b, C>(
        &'b self,
        clause: &'b Clause<'a, C>,
        input: &'b [ConstantRef<'a, C>],
        params: &'b [Option<ConstantRef<'a, C>>],
        tup: &'b impl Fn(usize) -> ConstantRef<'a, C>,
    ) -> impl Iterator<Item = ConstantRef<'a, C>> + 'b {
        self.output.iter().map(move |col| match *col {
            Column::Input(idx) => input[idx],
            Column::Tuple(c) => tup(c),
            Column::ParameterOrTuple { slot, column } => {
                params[slot].unwrap_or_else(|| tup(column))
            }
            Column::HeadConstant(idx) => ConstantRef(clause.head_constant(idx)),
        })
    }
}
