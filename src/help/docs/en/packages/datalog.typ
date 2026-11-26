#show: package(
  summary: [Evaluate stratified Datalog queries over site relations, facts, and derivation rules.],
)

Declare each relation (including empty ones) with `relation`, supply ground rows with `.atom(...)`, derive new tuples with `rule`, and query the resulting `database`.

- *Range restriction*: every variable used in a rule head, `select`, or `negate(...)` must also appear in at least one positive literal in the same rule or query.
- *Stratified negation*: a relation may appear inside `negate(...)` only when its definition does not depend (directly or indirectly) on the head relation.
- *Deterministic ordering*: query results are deduplicated and sorted column-by-column. Distinct types compare in fixed order: `none`, `bool`, `int`, finite `float`, `decimal`, `str`, `bytes`, `datetime` (numeric types such as `1` and `1.0` stay distinct).

#export(
  "relation",
  summary: [Declare a named relation with a fixed number of columns.],
  params: (
    name: [Non-empty, case-sensitive relation name.],
    arity: [Non-negative integer column count.],
  ),
  related: ("database", "rule", "var"),
)[
  Call `.atom(...)` on the returned relation to build facts or rule/query literals.
]

#method(
  "atom",
  summary: [Construct a fact row or query pattern matching this relation's arity.],
  params: (
    terms: [One constant or `var(...)` per column; omit for zero-arity relations. Represent Typst content or functions by string IDs.],
  ),
)

#export(
  "var",
  summary: [Create a logical variable scoped to a rule or query.],
  params: (
    name: [Non-empty, case-sensitive variable name. Equal names within one rule or query refer to the same variable.],
  ),
  related: ("relation", "rule", "database"),
)

#export(
  "rule",
  summary: [Derive `head` whenever every body literal holds.],
  params: (
    head: [Derived atom produced when all conditions match.],
    literal: [First body condition (a positive atom or `negate(...)`).],
    rest: [Zero or more additional body conditions.],
  ),
  related: ("negate", "database"),
)

#export(
  "negate",
  summary: [Succeed when no row in the relation matches `atom`.],
  params: (
    atom: [Atom to test for absence.],
  ),
  related: ("rule", "database"),
)

#export(
  "database",
  summary: [Build a Datalog database from declared relations, ground facts, and rules.],
  params: (
    relations: [Array of unique `relation(...)` declarations, including relations with no initial facts.],
    facts: [Ground fact rows (constants only). Defaults to `()`.],
    rules: [Derivation rules built with `rule(...)`. Defaults to `()`.],
  ),
  related: ("relation", "rule", "var", "negate"),
)[
  Call `db.query(select: (...), where: (...))` on the returned database. An empty `select` returns `((),)` when the query holds and `()` otherwise; an empty `where` always holds.

  #example(title: [Find reachable pages while excluding hidden ones], check: "default")[
  ```typst
  #import "@tola/datalog:0.0.0": database, negate, relation, rule, var

  #let edge = relation("edge", 2)
  #let reach = relation("reach", 2)
  #let hidden = relation("hidden", 1)
  #let (x, y, z) = (var("x"), var("y"), var("z"))

  #let db = database(
    (edge, reach, hidden),
    facts: (edge.atom("a", "b"), edge.atom("b", "c"), hidden.atom("b")),
    rules: (
      rule(reach.atom(x, y), edge.atom(x, y)),
      rule(reach.atom(x, z), reach.atom(x, y), edge.atom(y, z)),
    ),
  )

  #let visible = db.query(
    select: (z,),
    where: (reach.atom("a", z), negate(hidden.atom(z))),
  )
  #document("index.html")[Reachable: #visible.map(row => row.first()).join(", ")]
  ```
  ]
]

#method(
  "query",
  summary: [Return sorted, deduplicated result rows matching all conditions in `where`.],
  params: (
    select: [Array of `var(...)` values defining output columns (variables may repeat).],
    where: [Array of positive atoms and `negate(...)` conditions that must all hold (`()` matches unconditionally).],
  ),
)
