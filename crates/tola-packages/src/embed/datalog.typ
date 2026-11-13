// @tola/datalog:0.0.0 - relations, recursive rules, and queries
//
// Declare every relation in the database, including empty relations. An unknown
// relation is an error in facts, rules, and queries. A name identifies a relation
// within the database and has one arity.
//
// Facts are ground atoms. Variables in a rule head, query selection, or negated
// atom must appear in a positive atom of the same rule or query. A dependency
// cycle cannot pass through negation.
//
// Constants of different types are distinct. Their order is none, bool, int,
// finite float, decimal, str, bytes, datetime. Within each type, booleans and
// numbers use value order, strings and bytes use lexicographic order, and
// datetimes order dates before times before date-times, each chronologically.
// Decimal values lose trailing fractional zeros. Both float and decimal zeros
// have one representation.

/// Declare a relation by name and arity. Use `.atom(..terms)` to construct an atom.
/// Arity can be zero.
#import "@tola/host:0.0.0": datalog-relation as relation

/// Create a nonempty variable name. Equal names identify the same variable within one rule or query.
#import "@tola/host:0.0.0": datalog-var as var

/// Derive the head atom when all body conditions hold. Pass at least one condition.
#import "@tola/host:0.0.0": datalog-rule as rule

/// Construct a literal that holds when the atom has no matching row for the current binding.
#import "@tola/host:0.0.0": datalog-negate as negate

/// Build an immutable database from relation declarations, ground atoms, and rules.
/// Each name must be declared exactly once. A relation may have facts and deriving rules.
/// Database equality compares declarations, facts, and the structure of rules.
///
/// `.query(select: ..., where: ...)` requires two arrays. The selection lists variables in
/// output column order and may repeat a variable. The conditions form a conjunction.
/// The result is an array of rows, sorted lexicographically and deduplicated using Datalog
/// constant equality. Typst's equality can compare values of different numeric types as equal.
///
/// An empty selection returns `((),)` when the conditions hold and `()` otherwise.
/// Empty conditions hold.
///
/// This example finds nodes reachable from `a` and excludes those in `hidden`. The two rows
/// are `("b",)` and `("c",)`.
///
/// ```typst
/// #import "@tola/datalog:0.0.0": relation, var, rule, negate, database
/// #let edge = relation("edge", 2)
/// #let reach = relation("reach", 2)
/// #let hidden = relation("hidden", 1)
/// #let x = var("x")
/// #let y = var("y")
/// #let z = var("z")
/// #let db = database((edge, reach, hidden),
///   facts: (edge.atom("a", "b"), edge.atom("b", "c")),
///   rules: (
///     rule(reach.atom(x, y), edge.atom(x, y)),
///     rule(reach.atom(x, z), reach.atom(x, y), edge.atom(y, z)),
///   ),
/// )
/// #assert.eq(db.query(select: (z,), where: (
///   reach.atom("a", z), negate(hidden.atom(z)),
/// )), (("b",), ("c",)))
/// ```
#import "@tola/host:0.0.0": datalog-database as database
