//! Native values and functions for `@tola/datalog`.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tola_datalog::{
    Atom, Database, Error, ErrorKind, Literal, Location, Query, Relation, Rule, Term,
};
use typst::diag::{SourceResult, bail};
use typst::ecow::EcoString;
use typst::foundations::{
    Args, Array, Bytes, Datetime, Decimal, Func, IntoValue, NativeFunc, Repr, Scope, Str, Value,
    func, ty,
};
use typst::syntax::{Span, Spanned};
use typst::utils::LazyHash;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Constant {
    None,
    Bool(bool),
    Int(i64),
    Float(FiniteFloat),
    Decimal(Decimal),
    Str(Str),
    Bytes(BytesValue),
    Date(DateValue),
}

#[derive(Clone, Debug)]
struct BytesValue(Bytes);
impl PartialEq for BytesValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_slice() == other.0.as_slice()
    }
}
impl Eq for BytesValue {}
impl PartialOrd for BytesValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for BytesValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.as_slice().cmp(other.0.as_slice())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FiniteFloat(u64);
impl PartialOrd for FiniteFloat {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FiniteFloat {
    fn cmp(&self, other: &Self) -> Ordering {
        f64::from_bits(self.0).total_cmp(&f64::from_bits(other.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DateValue(Datetime);
impl Eq for DateValue {}
impl PartialOrd for DateValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for DateValue {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.0, other.0) {
            (Datetime::Date(a), Datetime::Date(b)) => a.cmp(&b),
            (Datetime::Time(a), Datetime::Time(b)) => a.cmp(&b),
            (Datetime::Datetime(a), Datetime::Datetime(b)) => a.cmp(&b),
            (a, b) => date_kind(a).cmp(&date_kind(b)),
        }
    }
}
fn date_kind(value: Datetime) -> u8 {
    match value {
        Datetime::Date(_) => 0,
        Datetime::Time(_) => 1,
        Datetime::Datetime(_) => 2,
    }
}
impl Constant {
    // Joins compare decimals numerically. Cached Typst values also retain their scale.
    fn hash_repr<H: Hasher>(&self, hasher: &mut H) {
        std::mem::discriminant(self).hash(hasher);
        match self {
            Self::None => {}
            Self::Bool(v) => v.hash(hasher),
            Self::Int(v) => v.hash(hasher),
            Self::Float(v) => v.0.hash(hasher),
            Self::Decimal(v) => v.hash(hasher),
            Self::Str(v) => v.hash(hasher),
            Self::Bytes(v) => v.0.hash(hasher),
            Self::Date(v) => v.0.hash(hasher),
        }
    }

    fn from_value(value: &Value, span: Span) -> SourceResult<Self> {
        Ok(match value {
            Value::None => Self::None,
            Value::Bool(v) => Self::Bool(*v),
            Value::Int(v) => Self::Int(*v),
            Value::Float(v) if v.is_finite() => {
                Self::Float(FiniteFloat(if *v == 0.0 { 0 } else { v.to_bits() }))
            }
            Value::Float(_) => {
                bail!(span, "Datalog float terms must be finite"; hint: "use a finite number")
            }
            Value::Decimal(v) => Self::Decimal(*v),
            Value::Str(v) => Self::Str(v.clone()),
            Value::Bytes(v) => Self::Bytes(BytesValue(v.clone())),
            Value::Datetime(v) => Self::Date(DateValue(*v)),
            _ => {
                bail!(span, "Datalog terms must be variables or immutable scalar values"; hint: "use an ID for content, functions, or other complex values")
            }
        })
    }
    fn into_value(self) -> Value {
        match self {
            Self::None => Value::None,
            Self::Bool(v) => Value::Bool(v),
            Self::Int(v) => Value::Int(v),
            Self::Float(v) => Value::Float(f64::from_bits(v.0)),
            Self::Decimal(v) => Value::Decimal(v),
            Self::Str(v) => Value::Str(v),
            Self::Bytes(v) => Value::Bytes(v.0),
            Self::Date(v) => Value::Datetime(v.0),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedAtom {
    key: Arc<LazyHash<AtomKey>>,
    span: Span,
}
impl LocatedAtom {
    fn new(atom: Atom<Constant>, span: Span) -> Self {
        Self {
            key: Arc::new(LazyHash::new(AtomKey(atom))),
            span,
        }
    }

    fn as_atom(&self) -> &Atom<Constant> {
        &self.key.0
    }
}
#[derive(Debug)]
struct AtomKey(Atom<Constant>);
impl Hash for AtomKey {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.0.hash_with(hasher, Constant::hash_repr);
    }
}
#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedLiteral {
    atom: LocatedAtom,
    negative: bool,
}
impl LocatedLiteral {
    fn to_literal(&self) -> Literal<Constant> {
        let atom = self.atom.as_atom().clone();
        if self.negative {
            Literal::Negative(atom)
        } else {
            Literal::Positive(atom)
        }
    }
}
#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedRule {
    head: LocatedAtom,
    body: Vec<LocatedLiteral>,
}
struct DatabaseClauses<'a> {
    atoms: &'a [&'a LocatedAtom],
    rules: &'a [&'a LocatedRule],
}
impl Hash for DatabaseClauses<'_> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        // Cached errors use input positions; keys preserve clause order and omit spans.
        self.atoms.len().hash(hasher);
        for atom in self.atoms {
            atom.key.hash(hasher);
        }
        self.rules.len().hash(hasher);
        for rule in self.rules {
            rule.head.key.hash(hasher);
            hash_body(&rule.body, hasher);
        }
    }
}
#[derive(Debug)]
struct Snapshot(Database<Constant>);
impl Hash for Snapshot {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.0.hash_with(hasher, Constant::hash_repr);
    }
}
struct QueryClauses<'a> {
    select: Vec<String>,
    body: &'a [LocatedLiteral],
}
impl Hash for QueryClauses<'_> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.select.hash(hasher);
        hash_body(self.body, hasher);
    }
}
fn hash_body<H: Hasher>(body: &[LocatedLiteral], hasher: &mut H) {
    body.len().hash(hasher);
    for literal in body {
        literal.negative.hash(hasher);
        literal.atom.key.hash(hasher);
    }
}
/// A value used by the Datalog package.
#[ty]
#[derive(Clone, Debug, Hash, PartialEq)]
enum DatalogValue {
    Variable(String, Span),
    Atom(LocatedAtom),
    Literal(LocatedLiteral),
    Rule(LocatedRule),
    Database(Arc<LazyHash<Snapshot>>),
}
impl Repr for DatalogValue {
    fn repr(&self) -> EcoString {
        match self {
            Self::Variable(name, _) => {
                format!("<variable {}>", Str::from(name.as_str()).repr()).into()
            }
            Self::Atom(atom) => atom_repr("atom", &atom.as_atom().relation),
            Self::Literal(literal) => atom_repr("negated atom", &literal.atom.as_atom().relation),
            Self::Rule(rule) => atom_repr("rule for", &rule.head.as_atom().relation),
            Self::Database(_) => "<datalog database>".into(),
        }
    }
}
fn atom_repr(kind: &str, relation: &Relation) -> EcoString {
    format!(
        "<{kind} {} / {}>",
        Str::from(relation.name.as_str()).repr(),
        relation.arity
    )
    .into()
}
fn wrapped(value: DatalogValue) -> Value {
    Value::dynamic(value)
}
fn datalog(value: &Value) -> Option<&DatalogValue> {
    match value {
        Value::Dyn(v) => v.downcast(),
        _ => None,
    }
}
fn get_atom(value: &Value, span: Span) -> SourceResult<&LocatedAtom> {
    match datalog(value) {
        Some(DatalogValue::Atom(v)) => Ok(v),
        _ => bail!(span, "expected a Datalog atom"),
    }
}
fn literal(value: &Value, span: Span) -> SourceResult<LocatedLiteral> {
    match datalog(value) {
        Some(DatalogValue::Atom(v)) => Ok(LocatedLiteral {
            atom: v.clone(),
            negative: false,
        }),
        Some(DatalogValue::Literal(v)) => Ok(v.clone()),
        _ => bail!(span, "expected a Datalog atom or negated atom"),
    }
}

#[func]
fn datalog_relation(
    /// Exact nonempty relation name.
    name: Spanned<Str>,
    /// Number of terms, at least zero.
    arity: Spanned<i64>,
) -> SourceResult<Func> {
    if name.v.is_empty() {
        bail!(name.span, "relation name must not be empty")
    }
    let Ok(arity_value) = usize::try_from(arity.v) else {
        bail!(arity.span, "relation arity must be nonnegative")
    };
    let mut args = Args::new(
        arity.span,
        [name.v.into_value(), (arity_value as i64).into_value()],
    );
    Ok(Func::from(datalog_atom::data()).with(&mut args))
}
#[func]
fn datalog_atom(
    args: &mut Args,
    /// Terms in the relation.
    #[external]
    #[variadic]
    terms: Vec<Value>,
) -> SourceResult<Value> {
    let span = args.span;
    let name: Str = args.expect("name")?;
    let arity: i64 = args.expect("arity")?;
    let terms: Vec<Spanned<Value>> = args.all()?;
    if terms.len() != arity as usize {
        bail!(
            span,
            "relation `{name}` expects {arity} terms, found {}",
            terms.len()
        );
    }
    let terms = terms
        .into_iter()
        .map(|term| {
            if let Some(DatalogValue::Variable(name, _)) = datalog(&term.v) {
                Ok(Term::Variable(name.clone()))
            } else {
                Constant::from_value(&term.v, term.span).map(Term::Constant)
            }
        })
        .collect::<SourceResult<Vec<_>>>()?;
    Ok(wrapped(DatalogValue::Atom(LocatedAtom::new(
        Atom {
            relation: Relation {
                name: name.to_string(),
                arity: arity as usize,
            },
            terms,
        },
        span,
    ))))
}
#[func]
fn datalog_var(
    span: Span,
    /// Exact variable name.
    name: Str,
) -> Value {
    wrapped(DatalogValue::Variable(name.to_string(), span))
}
#[func]
fn datalog_negate(
    /// Atom that must have no matching row.
    atom: Spanned<Value>,
) -> SourceResult<Value> {
    let atom = get_atom(&atom.v, atom.span)?;
    Ok(wrapped(DatalogValue::Literal(LocatedLiteral {
        atom: atom.clone(),
        negative: true,
    })))
}
#[func]
fn datalog_rule(
    /// Atom to derive.
    head: Spanned<Value>,
    /// Body literals, passed as positional arguments.
    #[variadic]
    body: Vec<Spanned<Value>>,
) -> SourceResult<Value> {
    let head = get_atom(&head.v, head.span)?.clone();
    let body = body
        .iter()
        .map(|v| literal(&v.v, v.span))
        .collect::<SourceResult<Vec<_>>>()?;
    Ok(wrapped(DatalogValue::Rule(LocatedRule { head, body })))
}
#[func]
fn datalog_database(
    span: Span,
    /// Ground atoms, default `()`.
    #[named]
    #[default(Spanned::detached(Array::new()))]
    facts: Spanned<Array>,
    /// Rules, default `()`.
    #[named]
    #[default(Spanned::detached(Array::new()))]
    rules: Spanned<Array>,
) -> SourceResult<Value> {
    let mut atoms = Vec::with_capacity(facts.v.len());
    for (index, value) in facts.v.iter().enumerate() {
        let Some(DatalogValue::Atom(atom)) = datalog(value) else {
            bail!(
                facts.span,
                "entry {} in `facts` must be a Datalog atom",
                index + 1
            );
        };
        if atom
            .as_atom()
            .terms
            .iter()
            .any(|term| matches!(term, Term::Variable(_)))
        {
            bail!(atom.span, "database facts must contain only constants");
        }
        atoms.push(atom);
    }
    let mut definitions = Vec::with_capacity(rules.v.len());
    for (index, value) in rules.v.iter().enumerate() {
        let Some(DatalogValue::Rule(rule)) = datalog(value) else {
            bail!(
                rules.span,
                "entry {} in `rules` must be a Datalog rule",
                index + 1
            );
        };
        definitions.push(rule);
    }
    let clauses = DatabaseClauses {
        atoms: &atoms,
        rules: &definitions,
    };
    let database = database_snapshot(clauses)
        .map_err(|e| diagnostic(e, &atoms, &definitions, &[], &[], span))?;
    Ok(wrapped(DatalogValue::Database(database)))
}

#[comemo::memoize]
fn database_snapshot(clauses: DatabaseClauses<'_>) -> Result<Arc<LazyHash<Snapshot>>, Error> {
    let rows = clauses.atoms.iter().map(|atom| {
        let atom = atom.as_atom();
        let values = atom.terms.iter().map(|term| match term {
            Term::Constant(value) => value.clone(),
            Term::Variable(_) => unreachable!(),
        });
        (&atom.relation, values)
    });
    let rules = clauses
        .rules
        .iter()
        .map(|rule| Rule {
            head: rule.head.as_atom().clone(),
            body: rule.body.iter().map(LocatedLiteral::to_literal).collect(),
        })
        .collect();
    Database::from_rows(rows, rules).map(|database| Arc::new(LazyHash::new(Snapshot(database))))
}

#[func]
fn datalog_query(
    span: Span,
    /// Database to query.
    database: Spanned<Value>,
    /// Variables in output column order.
    #[named]
    #[default(Spanned::detached(None))]
    select: Spanned<Option<Array>>,
    /// Body literals to match.
    #[named]
    #[default(Spanned::detached(None))]
    where_: Spanned<Option<Array>>,
) -> SourceResult<Array> {
    let Some(DatalogValue::Database(db)) = datalog(&database.v) else {
        bail!(database.span, "expected a Datalog database")
    };
    let Some(selection) = select.v else {
        bail!(span, "query requires `select`")
    };
    let Some(literals) = where_.v else {
        bail!(span, "query requires `where`")
    };
    let mut names = Vec::with_capacity(selection.len());
    let mut select_spans = Vec::with_capacity(selection.len());
    for (index, value) in selection.iter().enumerate() {
        let Some(DatalogValue::Variable(name, span)) = datalog(value) else {
            bail!(
                select.span,
                "entry {} in `select` must be a Datalog variable",
                index + 1
            );
        };
        names.push(name.clone());
        select_spans.push(*span);
    }
    let body = literals
        .iter()
        .map(|v| literal(v, where_.span))
        .collect::<SourceResult<Vec<_>>>()?;
    let clauses = QueryClauses {
        select: names,
        body: &body,
    };
    query_rows(db, clauses).map_err(|e| diagnostic(e, &[], &[], &body, &select_spans, span))
}
// The compile boundary handles cancellation so Typst cannot cache a cancelled query.
#[comemo::memoize]
fn query_rows(database: &LazyHash<Snapshot>, clauses: QueryClauses<'_>) -> Result<Array, Error> {
    let query = Query {
        select: clauses.select,
        body: clauses
            .body
            .iter()
            .map(LocatedLiteral::to_literal)
            .collect(),
    };
    let rows = database.0.query(&query, || false)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(Constant::into_value)
                .collect::<Array>()
                .into_value()
        })
        .collect())
}

fn diagnostic(
    error: Error,
    facts: &[&LocatedAtom],
    rules: &[&LocatedRule],
    body: &[LocatedLiteral],
    select: &[Span],
    fallback: Span,
) -> typst::ecow::EcoVec<typst::diag::SourceDiagnostic> {
    let span = match error.location {
        Location::Fact(i) => facts.get(i).map(|v| v.span),
        Location::RuleHead(i) => rules.get(i).map(|v| v.head.span),
        Location::RuleBody { rule, atom } => rules
            .get(rule)
            .and_then(|r| r.body.get(atom))
            .map(|v| v.atom.span),
        Location::QueryBody(i) => body.get(i).map(|v| v.atom.span),
        Location::QuerySelect(i) => select.get(i).copied(),
        _ => None,
    }
    .unwrap_or(fallback);
    let negative_cycle = matches!(error.kind, ErrorKind::NegativeCycle);
    let message = match error.kind {
        ErrorKind::Arity { expected, actual } => {
            format!("relation expects {expected} terms, found {actual}")
        }
        ErrorKind::RelationArity {
            name,
            expected,
            actual,
        } => format!("relation `{name}` has arity {actual}, expected {expected}"),
        ErrorKind::UnboundVariable(name) => {
            format!("variable `{name}` must occur in a positive body atom")
        }
        ErrorKind::EmptyRelation => "relation name must not be empty".to_owned(),
        ErrorKind::NegativeCycle => match error.location {
            Location::RuleBody { rule, atom } => rules
                .get(rule)
                .and_then(|r| r.body.get(atom).map(|body| (r, body)))
                .map(|(r, body)| {
                    let relation = &body.atom.as_atom().relation.name;
                    format!(
                        "rule for `{}` has a cycle through negation involving `{relation}`",
                        r.head.as_atom().relation.name
                    )
                })
                .unwrap_or_else(|| "rules contain a cycle through negation".to_owned()),
            _ => "rules contain a cycle through negation".to_owned(),
        },
        ErrorKind::Cancelled => "Datalog query was cancelled".to_owned(),
    };
    let mut diagnostic = typst::diag::SourceDiagnostic::error(span, message);
    if negative_cycle {
        diagnostic.hint("remove the dependency cycle through negation");
    }
    typst::ecow::eco_vec![diagnostic]
}

pub(crate) fn define(scope: &mut Scope) {
    scope.define("datalog-relation", Func::from(datalog_relation::data()));
    scope.define("datalog-var", Func::from(datalog_var::data()));
    scope.define("datalog-rule", Func::from(datalog_rule::data()));
    scope.define("datalog-negate", Func::from(datalog_negate::data()));
    scope.define("datalog-database", Func::from(datalog_database::data()));
    scope.define("datalog-query", Func::from(datalog_query::data()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relation_constructor_exposes_only_terms() {
        let relation =
            datalog_relation(Spanned::detached("edge".into()), Spanned::detached(2)).unwrap();
        let parameters = relation.params().collect::<Vec<_>>();
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].name(), Some("terms"));
        assert!(parameters[0].variadic());
    }
}
