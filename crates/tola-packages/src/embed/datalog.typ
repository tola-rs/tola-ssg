// @tola/datalog:0.0.0 - relations, recursive rules, and queries
//
// Use Datalog to query relationships that take more than one step to follow. For example,
// write down the direct links between pages, then query which pages you can reach by
// following any number of links.
//
// A relation names a table and sets its column count (arity). A fact is a row of
// constants, such as `edge.atom("a", "b")`. A rule derives a row when its conditions
// hold. Declare every relation, including empty ones, before using it.
//
// A variable in the rule head, in `select`, or inside `negate` must also occur in an
// positive atom in the same rule or query, such as `reach.atom("a", z)`.
// That atom supplies the values to try.
// Recursive rules can follow links. A rule defining `visible` can use
// `negate(hidden.atom(z))` only if `hidden` does not depend on `visible`, even through
// other relations. This is stratified negation.
//
// Query rows are sorted by column and deduplicated. Different types stay distinct.
// The type order is `none`, `bool`, `int`, finite `float`, `decimal`, `str`, `bytes`,
// and `datetime`. Strings and bytes sort lexicographically. Dates, times, and
// date-times sort separately in that order.

/// Declare a relation by name and column count. For example, `relation("edge", 2)`
/// declares two columns for the start and end of a link. Use `atom` to write a fact
/// or match rows in a rule or query.
#import "@tola/host:0.0.0": datalog-relation as relation

/// Create a logic variable. Within a rule or query, the same name refers to the same variable.
#import "@tola/host:0.0.0": datalog-var as var

/// Add the row described by `head` when every condition holds. Pass at least one condition.
#import "@tola/host:0.0.0": datalog-rule as rule

/// Match when the relation has no row for the given values.
#import "@tola/host:0.0.0": datalog-negate as negate

/// Create a database from relation declarations, known facts, and rules. Use each relation name once.
///
/// Call `db.query(select: ..., where: ...)` to read rows from the database. For example,
/// with the fact `edge.atom("a", "b")` and the variable `target = var("target")`, the query
/// `db.query(select: (target,), where: (edge.atom("a", target),))` returns `(("b",),)`.
///
/// Rows are sorted by the selected columns and deduplicated. Datalog treats numeric types
/// as distinct: `1` and `1.0` are different values.
///
/// An empty selection returns `((),)` when the conditions hold and `()` otherwise.
/// Empty conditions hold.
///
/// Example: Query reachable pages:
///
/// Replace `site.typ` in a new site with this example and run `tola build` from the site root.
/// The rules add direct and indirect paths to `reach`. The query filters out `b` because
/// it is in `hidden`. The path through `b` still reaches `c`, so `public/index.html`
/// displays `Reachable pages: c` with the default publish directory.
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
///   facts: (edge.atom("a", "b"), edge.atom("b", "c"), hidden.atom("b")),
///   rules: (
///     rule(reach.atom(x, y), edge.atom(x, y)),
///     rule(reach.atom(x, z), reach.atom(x, y), edge.atom(y, z)),
///   ),
/// )
/// #let rows = db.query(select: (z,), where: (
///   reach.atom("a", z), negate(hidden.atom(z)),
/// ))
/// #assert.eq(rows, (("c",),))
/// #document("index.html")[Reachable pages: #rows.map(row => row.first()).join(", ")]
/// ```
#import "@tola/host:0.0.0": datalog-database as database
