//! Native values and functions for `@tola/datalog`.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

use tola_datalog::{
    Atom, Database, Error, ErrorKind, Literal, Location, Query, Relation, Rule, Term,
};
use typst::diag::{SourceResult, bail, error};
use typst::ecow::{EcoString, eco_vec};
use typst::foundations::{
    Args, Array, Bytes, Datetime, Decimal, Func, IntoValue, NativeFunc, NativeFuncData,
    NativeFuncPtr, NativeParamInfo, NativeScope, NativeType, Reflect, Repr, Scope, Str, Type,
    Value, func, repr, scope, ty,
};
use typst::syntax::{Span, Spanned};
use typst::utils::{DefSite, LazyHash};

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
impl Hash for Constant {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
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
            Value::Decimal(v) => Self::Decimal(normalize_decimal(*v)),
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

fn normalize_decimal(value: Decimal) -> Decimal {
    if value.is_zero() {
        return Decimal::ZERO;
    }
    // Typst exposes exact decimal text, but no scale-normalization API.
    let text = value.to_string();
    let trimmed = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    trimmed
        .replace(repr::MINUS_SIGN, "-")
        .parse()
        .expect("exact decimal")
}

#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedAtom {
    key: Arc<LazyHash<Atom<Constant>>>,
    span: Span,
}
impl LocatedAtom {
    fn new(atom: Atom<Constant>, span: Span) -> Self {
        Self {
            key: Arc::new(LazyHash::new(atom)),
            span,
        }
    }

    fn as_atom(&self) -> &Atom<Constant> {
        &self.key
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
    relations: &'a [LocatedRelation],
    atoms: &'a [&'a LocatedAtom],
    rules: &'a [&'a LocatedRule],
}
impl Hash for DatabaseClauses<'_> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        // Cached errors use input positions; keys preserve clause order and omit spans.
        self.relations.len().hash(hasher);
        for relation in self.relations {
            relation.relation.hash(hasher);
        }
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
fn body_literal(value: &Value, span: Span) -> SourceResult<LocatedLiteral> {
    match datalog(value) {
        Some(DatalogValue::Atom(v)) => Ok(LocatedLiteral {
            atom: v.clone(),
            negative: false,
        }),
        Some(DatalogValue::Literal(v)) => Ok(v.clone()),
        _ => bail!(span, "expected a Datalog atom or negated atom"),
    }
}

/// A declaration of a Datalog relation.
#[ty(scope, name = "datalog-relation")]
#[derive(Clone, Debug, Hash, PartialEq)]
struct LocatedRelation {
    relation: Relation,
    span: Span,
}

impl Repr for LocatedRelation {
    fn repr(&self) -> EcoString {
        atom_repr("relation", &self.relation)
    }
}

#[func]
fn datalog_relation(
    span: Span,
    /// A nonempty, case-sensitive relation name.
    name: Spanned<Str>,
    /// Number of columns. A nonnegative integer.
    arity: Spanned<i64>,
) -> SourceResult<LocatedRelation> {
    if name.v.is_empty() {
        bail!(name.span, "relation name must not be empty")
    }
    let Ok(arity_value) = usize::try_from(arity.v) else {
        bail!(arity.span, "relation arity is out of range"; hint: "use a nonnegative integer")
    };
    Ok(LocatedRelation {
        relation: Relation {
            name: name.v.to_string(),
            arity: arity_value,
        },
        span,
    })
}

#[scope]
impl LocatedRelation {
    /// Construct an atom of this relation.
    #[func]
    fn atom(
        self,
        span: Span,
        /// Pass one constant or variable per column. For `edge = relation("edge", 2)`,
        /// `edge.atom("a", "b")` can go in `facts`. In a query, `edge.atom("a", var("target"))`
        /// matches links from `"a"`. For a zero-column relation, call `atom()`.
        #[variadic]
        terms: Vec<Spanned<Value>>,
    ) -> SourceResult<Value> {
        let Relation { name, arity } = &self.relation;
        if terms.len() != *arity {
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
                relation: self.relation,
                terms,
            },
            span,
        ))))
    }
}

#[func]
fn datalog_var(
    /// A nonempty, case-sensitive variable name.
    name: Spanned<Str>,
) -> SourceResult<Value> {
    if name.v.is_empty() {
        bail!(name.span, "variable name must not be empty")
    }
    Ok(wrapped(DatalogValue::Variable(
        name.v.to_string(),
        name.span,
    )))
}

#[func]
fn datalog_negate(
    /// The atom to look up. The condition holds if no row matches.
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
    /// The first condition, a positive atom or a `negate` condition.
    literal: Spanned<Value>,
    /// Additional conditions, passed one at a time.
    #[variadic]
    rest: Vec<Spanned<Value>>,
) -> SourceResult<Value> {
    let head = get_atom(&head.v, head.span)?.clone();
    let body = std::iter::once(literal)
        .chain(rest)
        .map(|v| body_literal(&v.v, v.span))
        .collect::<SourceResult<Vec<_>>>()?;
    Ok(wrapped(DatalogValue::Rule(LocatedRule { head, body })))
}
#[func]
fn datalog_database(
    span: Span,
    /// Relation declarations, including relations with no facts or rules.
    relations: Spanned<Array>,
    /// Known rows. Each row must contain only constants. Defaults to `()`.
    #[named]
    #[default(Spanned::detached(Array::new()))]
    facts: Spanned<Array>,
    /// Rules, default `()`.
    #[named]
    #[default(Spanned::detached(Array::new()))]
    rules: Spanned<Array>,
) -> SourceResult<DatalogDatabase> {
    let declarations = relations
        .v
        .iter()
        .enumerate()
        .map(|(index, value)| {
            match value {
                Value::Dyn(value) => value.downcast::<LocatedRelation>().cloned(),
                _ => None,
            }
            .ok_or_else(|| {
                eco_vec![error!(
                    relations.span,
                    "entry {} in `relations` must be a Datalog relation",
                    index + 1
                )]
            })
        })
        .collect::<SourceResult<Vec<_>>>()?;
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
        relations: &declarations,
        atoms: &atoms,
        rules: &definitions,
    };
    let database = database_snapshot(clauses).map_err(|e| {
        diagnostic(e, span, |location| match location {
            Location::Relation(i) => declarations.get(i).map(|v| v.span),
            Location::Fact(i) => atoms.get(i).map(|v| v.span),
            Location::RuleHead(i) => definitions.get(i).map(|v| v.head.span),
            Location::RuleBody { rule, atom } => definitions
                .get(rule)
                .and_then(|rule| rule.body.get(atom))
                .map(|literal| literal.atom.span),
            _ => None,
        })
    })?;
    Ok(database)
}

#[comemo::memoize]
fn database_snapshot(clauses: DatabaseClauses<'_>) -> Result<DatalogDatabase, Error> {
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
    let relations = clauses.relations.iter().map(|relation| &relation.relation);
    Database::from_rows(relations, rows, rules)
        .map(|database| DatalogDatabase(Arc::new(LazyHash::new(database))))
}

/// An immutable Datalog database.
#[ty(scope)]
#[derive(Clone, Debug, Hash, PartialEq)]
struct DatalogDatabase(Arc<LazyHash<Database<Constant>>>);

impl Repr for DatalogDatabase {
    fn repr(&self) -> EcoString {
        "<datalog database>".into()
    }
}

impl NativeScope for DatalogDatabase {
    fn scope() -> Scope {
        let mut scope = Scope::new();
        scope.define_func_with_data(&QUERY);
        scope
    }

    fn constructor() -> Option<&'static NativeFuncData> {
        None
    }
}

static QUERY: NativeFuncData = NativeFuncData {
    function: NativeFuncPtr(&|_, _, args| query(args).map(IntoValue::into_value)),
    name: "query",
    title: "Query",
    docs: "Return the distinct rows matching the conditions in this database.",
    def_site: Some(DefSite {
        path: file!(),
        key: "query",
    }),
    keywords: &[],
    contextual: false,
    scope: LazyLock::new(&Scope::new),
    params: LazyLock::new(&|| {
        let parameter = |name, docs, input, named| NativeParamInfo {
            name,
            docs,
            input,
            named,
            positional: !named,
            required: true,
            default: None,
            def_site: None,
            variadic: false,
            settable: false,
        };
        vec![
            parameter("self", "", DatalogDatabase::input(), false),
            parameter(
                "select",
                "Pass an array of variables in output column order. You can repeat a variable.",
                Array::input(),
                true,
            ),
            parameter(
                "where",
                "Pass an array of conditions. Each atom or `negate` condition must hold. Use `()` for no conditions.",
                Array::input(),
                true,
            ),
        ]
    }),
    returns: LazyLock::new(&Array::output),
};

fn query(args: &mut Args) -> SourceResult<Array> {
    let span = args.span;
    let database: DatalogDatabase = args.expect("self")?;
    let select: Spanned<Array> = args
        .named("select")?
        .ok_or_else(|| eco_vec![error!(span, "query requires `select`")])?;
    let where_: Spanned<Array> = args
        .named("where")?
        .ok_or_else(|| eco_vec![error!(span, "query requires `where`")])?;
    args.take().finish()?;
    let mut names = Vec::with_capacity(select.v.len());
    let mut select_spans = Vec::with_capacity(select.v.len());
    for (index, value) in select.v.iter().enumerate() {
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
    let body = where_
        .v
        .iter()
        .map(|v| body_literal(v, where_.span))
        .collect::<SourceResult<Vec<_>>>()?;
    let clauses = QueryClauses {
        select: names,
        body: &body,
    };
    query_rows(&database.0, clauses).map_err(|e| {
        diagnostic(e, span, |location| match location {
            Location::QueryBody(i) => body.get(i).map(|v| v.atom.span),
            Location::QuerySelect(i) => select_spans.get(i).copied(),
            _ => None,
        })
    })
}

pub(crate) fn methods(ty: Type) -> impl Iterator<Item = (&'static str, &'static Func)> {
    let scope = (ty == DatalogDatabase::ty() || ty == LocatedRelation::ty()).then(|| ty.scope());
    scope
        .into_iter()
        .flat_map(Scope::iter)
        .filter_map(|(name, binding)| match binding.read() {
            Value::Func(func) => Some((name.as_str(), func)),
            _ => None,
        })
}

// The compile boundary handles cancellation so Typst cannot cache a cancelled query.
#[comemo::memoize]
fn query_rows(
    database: &LazyHash<Database<Constant>>,
    clauses: QueryClauses<'_>,
) -> Result<Array, Error> {
    let query = Query {
        select: clauses.select,
        body: clauses
            .body
            .iter()
            .map(LocatedLiteral::to_literal)
            .collect(),
    };
    let rows = database.query(&query, || false)?;
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
    fallback: Span,
    locate: impl FnOnce(Location) -> Option<Span>,
) -> typst::ecow::EcoVec<typst::diag::SourceDiagnostic> {
    let span = locate(error.location).unwrap_or(fallback);
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
        ErrorKind::DuplicateRelation(name) => format!("relation `{name}` is already declared"),
        ErrorKind::UnknownRelation(name) => format!("relation `{name}` is not declared"),
        ErrorKind::EmptyRelation => "relation name must not be empty".to_owned(),
        ErrorKind::EmptyVariable => "variable name must not be empty".to_owned(),
        ErrorKind::EmptyBody => "a rule needs at least one body literal".to_owned(),
        ErrorKind::NegativeCycle => "rules contain a cycle through negation".to_owned(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_signature_requires_named_arrays() {
        let query = Func::from(&QUERY);
        let parameters = query.params().skip(1).collect::<Vec<_>>();
        assert_eq!(parameters.len(), 2);
        for (parameter, name) in parameters.into_iter().zip(["select", "where"]) {
            let parameter = parameter.to_native().unwrap();
            assert_eq!(parameter.name, name);
            assert!(parameter.named && parameter.required);
            assert_eq!(parameter.input, Array::input());
            assert!(parameter.default.is_none());
        }
    }
}
