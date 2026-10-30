//! Native values and functions for `@tola/datalog`.

use std::cmp::Ordering;
use std::hash::Hash;
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

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum Constant {
    None,
    Bool(bool),
    Int(i64),
    Float(FiniteFloat),
    Decimal(Decimal),
    Str(Str),
    Bytes(Vec<u8>),
    Date(DateValue),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Hash)]
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
            Value::Bytes(v) => Self::Bytes(v.as_slice().to_vec()),
            Value::Datetime(v) => Self::Date(DateValue(*v)),
            _ => {
                bail!(span, "Datalog terms must be variables or immutable scalar values"; hint: "use an ID for content, functions, or other complex values")
            }
        })
    }
    fn value(&self) -> Value {
        match self {
            Self::None => Value::None,
            Self::Bool(v) => Value::Bool(*v),
            Self::Int(v) => Value::Int(*v),
            Self::Float(v) => Value::Float(f64::from_bits(v.0)),
            Self::Decimal(v) => Value::Decimal(*v),
            Self::Str(v) => Value::Str(v.clone()),
            Self::Bytes(v) => Value::Bytes(Bytes::new(v.clone())),
            Self::Date(v) => Value::Datetime(v.0),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedAtom {
    atom: Atom<Constant>,
    span: Span,
}
#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedLiteral {
    literal: Literal<Constant>,
    span: Span,
}
#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedRule {
    head: LocatedAtom,
    body: Vec<LocatedLiteral>,
}
#[derive(Clone, Debug, Hash, PartialEq)]
struct StoredDatabase {
    database: Arc<LazyHash<Database<Constant>>>,
    facts: Vec<LocatedAtom>,
    rules: Vec<LocatedRule>,
}

/// A value used by the Datalog package.
#[ty]
#[derive(Clone, Debug, Hash, PartialEq)]
enum DatalogValue {
    Variable(String, Span),
    Atom(LocatedAtom),
    Literal(LocatedLiteral),
    Rule(LocatedRule),
    Database(Arc<LazyHash<StoredDatabase>>),
}
impl Repr for DatalogValue {
    fn repr(&self) -> EcoString {
        match self {
            Self::Variable(name, _) => {
                format!("<variable {}>", Str::from(name.as_str()).repr()).into()
            }
            Self::Atom(atom) => atom_repr("atom", &atom.atom.relation),
            Self::Literal(literal) => {
                let relation = match &literal.literal {
                    Literal::Positive(atom) | Literal::Negative(atom) => &atom.relation,
                };
                atom_repr("negated atom", relation)
            }
            Self::Rule(rule) => atom_repr("rule for", &rule.head.atom.relation),
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
fn get_atom(value: &Value, span: Span) -> SourceResult<LocatedAtom> {
    match datalog(value) {
        Some(DatalogValue::Atom(v)) => Ok(v.clone()),
        _ => bail!(span, "expected a Datalog atom"),
    }
}
fn literal(value: &Value, span: Span) -> SourceResult<LocatedLiteral> {
    match datalog(value) {
        Some(DatalogValue::Atom(v)) => Ok(LocatedLiteral {
            literal: Literal::Positive(v.atom.clone()),
            span: v.span,
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
    Ok(wrapped(DatalogValue::Atom(LocatedAtom {
        atom: Atom {
            relation: Relation {
                name: name.to_string(),
                arity: arity as usize,
            },
            terms,
        },
        span,
    })))
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
        literal: Literal::Negative(atom.atom),
        span: atom.span,
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
    let head = get_atom(&head.v, head.span)?;
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
    #[default]
    facts: Array,
    /// Rules, default `()`.
    #[named]
    #[default]
    rules: Array,
) -> SourceResult<Value> {
    let facts = facts
        .iter()
        .map(|v| get_atom(v, span))
        .collect::<SourceResult<Vec<_>>>()?;
    let rules = rules
        .iter()
        .map(|v| match datalog(v) {
            Some(DatalogValue::Rule(v)) => Ok(v.clone()),
            _ => bail!(span, "expected a Datalog rule"),
        })
        .collect::<SourceResult<Vec<_>>>()?;
    let mut ground = Vec::new();
    for fact in &facts {
        let mut values = Vec::new();
        for term in &fact.atom.terms {
            match term {
                Term::Constant(v) => values.push(v.clone()),
                Term::Variable(_) => bail!(fact.span, "database facts must contain only constants"),
            }
        }
        ground.push((fact.atom.relation.clone(), values));
    }
    let core_rules: Vec<_> = rules
        .iter()
        .map(|r| Rule {
            head: r.head.atom.clone(),
            body: r.body.iter().map(|v| v.literal.clone()).collect(),
        })
        .collect();
    let database = database_snapshot(&ground, &core_rules)
        .map_err(|e| diagnostic(e, &facts, &rules, &[], &[], span))?;
    Ok(wrapped(DatalogValue::Database(Arc::new(LazyHash::new(
        StoredDatabase {
            database,
            facts,
            rules,
        },
    )))))
}

#[comemo::memoize]
fn database_snapshot(
    facts: &[(Relation, Vec<Constant>)],
    rules: &[Rule<Constant>],
) -> Result<Arc<LazyHash<Database<Constant>>>, Error> {
    Database::new(facts.to_vec(), rules.to_vec()).map(|database| Arc::new(LazyHash::new(database)))
}

#[func]
fn datalog_query(
    span: Span,
    /// Database to query.
    database: Spanned<Value>,
    /// Variables in output column order.
    #[named]
    select: Option<Array>,
    /// Body literals to match.
    #[named]
    where_: Option<Array>,
) -> SourceResult<Array> {
    let Some(DatalogValue::Database(db)) = datalog(&database.v) else {
        bail!(database.span, "expected a Datalog database")
    };
    let Some(select) = select else {
        bail!(span, "query requires `select`")
    };
    let Some(where_) = where_ else {
        bail!(span, "query requires `where`")
    };
    let select_spans = select
        .iter()
        .map(|v| match datalog(v) {
            Some(DatalogValue::Variable(_, span)) => *span,
            _ => span,
        })
        .collect::<Vec<_>>();
    let select = select
        .iter()
        .map(|v| match datalog(v) {
            Some(DatalogValue::Variable(name, _)) => Ok(name.clone()),
            _ => bail!(span, "query selection must contain variables"),
        })
        .collect::<SourceResult<Vec<_>>>()?;
    let body = where_
        .iter()
        .map(|v| literal(v, span))
        .collect::<SourceResult<Vec<_>>>()?;
    let query = Query {
        select,
        body: body.iter().map(|v| v.literal.clone()).collect(),
    };
    let rows = query_rows(&db.database, &query)
        .map_err(|e| diagnostic(e, &db.facts, &db.rules, &body, &select_spans, span))?;
    Ok(rows
        .iter()
        .map(|row| {
            row.iter()
                .map(Constant::value)
                .collect::<Array>()
                .into_value()
        })
        .collect())
}
// Native Typst evaluation observes cancellation at the enclosing adapter boundary.
#[comemo::memoize]
fn query_rows(
    database: &LazyHash<Database<Constant>>,
    query: &Query<Constant>,
) -> Result<Vec<Vec<Constant>>, Error> {
    database.query(query, || false)
}

fn diagnostic(
    error: Error,
    facts: &[LocatedAtom],
    rules: &[LocatedRule],
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
            .map(|v| v.span),
        Location::QueryBody(i) => body.get(i).map(|v| v.span),
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
                    let relation = match &body.literal {
                        Literal::Positive(atom) | Literal::Negative(atom) => &atom.relation.name,
                    };
                    format!(
                        "rule for `{}` has a cycle through negation involving `{relation}`",
                        r.head.atom.relation.name
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
