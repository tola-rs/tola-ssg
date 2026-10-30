// @tola/datalog:0.0.0 - relations, recursive rules, and queries
//
// Relations have a nonempty name and fixed arity. Strings are constants; use `var` for variables.
// Facts are ground atoms. Rules and queries use set semantics, so duplicate rows occur once.
// Variables in a rule or query must occur in a positive body atom to be used
// in a head, selection, or negative atom. Negation must be stratified: no dependency cycle
// may contain a negative edge. Results are arrays of tuples.
//
// Constants are `none`, booleans, integers, finite floats, decimals, strings, bytes, and
// datetimes. Types remain distinct (for example `1` and `1.0`); use IDs to join complex
// values such as content and functions, then look those values up in your own records.

/// Create a function that constructs atoms of a named relation.
///
/// Relations with the same name in one database or query must have the same arity.
#import "@tola/host:0.0.0": datalog-relation as relation

/// Create a variable. Equal names identify the same variable within one rule or query.
#import "@tola/host:0.0.0": datalog-var as var

/// Create a rule from a head atom and zero or more positive or negative body literals.
#import "@tola/host:0.0.0": datalog-rule as rule

/// Make a negative body literal from an atom.
#import "@tola/host:0.0.0": datalog-negate as negate

/// Build an immutable database from ground atoms and rules.
///
/// Example - compute reachability through a recursive rule:
///
/// ```typst
/// #import "@tola/datalog:0.0.0": relation, var, rule, database, query
/// #let edge = relation("edge", 2)
/// #let reach = relation("reach", 2)
/// #let x = var("x")
/// #let y = var("y")
/// #let z = var("z")
/// #let db = database(facts: (edge("a", "b"), edge("b", "c")), rules: (
///   rule(reach(x, y), edge(x, y)),
///   rule(reach(x, z), reach(x, y), edge(y, z)),
/// ))
/// #assert(query(db, select: (z,), where: (reach("a", z),)).contains(("c",)))
/// ```
///
#import "@tola/host:0.0.0": datalog-database as database

/// Query a database using positive and negative literals.
///
/// Example - join references to records and exclude withdrawn references:
///
/// ```typst
/// #import "@tola/datalog:0.0.0": relation, var, negate, database, query
/// #let cites = relation("cites", 2)
/// #let title = relation("title", 2)
/// #let withdrawn = relation("withdrawn", 1)
/// #let id = var("id")
/// #let name = var("name")
/// #let db = database(facts: (
///   cites("essay", "r1"), cites("essay", "r2"),
///   title("r1", "Logic"), title("r2", "Old"), withdrawn("r2"),
/// ))
/// #assert.eq(query(db, select: (name,), where: (
///   cites("essay", id), title(id, name), negate(withdrawn(id)),
/// )), (("Logic",),))
/// ```
///
/// `select` contains variables, in output column order. An empty selection returns `((),)`
/// when the body succeeds, otherwise `()`. Results contain no duplicate rows.
#import "@tola/host:0.0.0": datalog-query as query
