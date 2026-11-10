use super::{assert_package, reject_package};

fn assert_schema(assertions: &str) {
    assert_package("schema", assertions);
}

fn reject_schema(assertions: &str) {
    reject_package("schema", assertions);
}

#[test]
fn partial_date_preserves_declared_shapes() {
    assert_schema(
        r#"
#let shapes = (
  (precision: "year", year: 2026),
  (precision: "month", year: 2026, month: 11),
  (precision: "day", year: 2024, month: 2, day: 29),
)
#for (source, expected) in ("2026", "2026-11", "2024-02-29").zip(shapes) {
  assert.eq(parse(source, partial-date), expected)
}
#assert.eq(parse(datetime(year: 2024, month: 2, day: 29), partial-date), shapes.last())
#let description = inspect(partial-date)
#assert.eq((description.kind, description.tag-key, description.tags), ("variant", "precision", ("year", "month", "day")))
#for (branch, expected) in description.branches.zip(shapes) {
  assert.eq(branch.kind, "object")
  assert.eq(branch.unknown, "error")
  assert.eq(branch.fields.keys().sorted(), expected.keys().sorted())
  assert.eq(branch.fields.precision.value, expected.precision)
  for (key, field) in branch.fields {
    assert.eq(field.presence, "required")
    if key != "precision" { assert.eq(field.type, int) }
  }
}
"#,
    );
}

#[test]
fn partial_date_collects_member_errors() {
    assert_schema(
        r#"
#let declaration = schema((dates: array-of(partial-date), title: str))
#let failed = try-parse((dates: ("2026", "2026-02-30", "0000"), title: false), declaration)
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (
  ("schema.partial-date", ("dates", 1)),
  ("schema.partial-date", ("dates", 2)),
  ("schema.type", ("title",)),
))
#for value in (none, 2026, datetime(hour: 8, minute: 30, second: 0), datetime(year: 2026, month: 11, day: 1, hour: 8, minute: 30, second: 0)) {
  assert.eq(try-parse(value, partial-date).ok, false)
}
"#,
    );
}

#[test]
fn optional_distinguishes_absence() {
    assert_schema(
        r#"
#let field = schema((value: optional(any, default: "fallback")))
#assert.eq(parse((:), field), (value: "fallback"))
#for value in (none, auto, false, 0, "", (), (:)) {
  assert.eq(parse((value: value), field), (value: value))
}
#assert.eq(parse((:), schema((value: optional(any)))), (:))
#assert.eq(parse((:), schema((value: optional(any, default: none)))), (value: none))
#let missing = try-parse((:), schema((value: any)))
#assert.eq(missing.issues.map(problem => (problem.code, problem.path)), (("schema.missing", ("value",)),))

#let label = optional(str)
#let child = schema((label: label))
#assert.eq(parse((child: (:)), schema((child: child))), (child: (:)))
#assert.eq(parse((:), schema((child: optional(child)))), (:))
#assert.eq(try-parse((:), schema((child: child))).issues.map(problem => problem.path), (("child",),))

#let absent = optional(str)
#for wrapped in (
  map(absent, _ => panic("map received absence")),
  convert(absent, _ => panic("convert received absence")),
  refine(absent, _ => panic("refine received absence"), message: "not reached"),
  check(absent, _ => panic("check received absence")),
  trim(absent),
  non-empty(absent),
  describe(absent, "Optional title"),
) {
  assert.eq(parse((:), schema((title: wrapped))), (:))
}
"#,
    );
}

#[test]
fn defaults_use_the_current_context() {
    assert_schema(
        r#"
#let page = schema((size: optional(map(str, _ => text.size), default: "local")))
#document("small.html")[
  #set text(size: 10pt)
  #context assert.eq(parse((:), page), (size: 10pt))
]
#document("large.html")[
  #set text(size: 20pt)
  #context assert.eq(parse((:), page), (size: 20pt))
]
"#,
    );
}

#[test]
fn invalid_input_never_uses_default() {
    assert_schema(
        r#"
#let page = schema((title: optional(str, default: "Untitled")))
#for value in (none, auto, false, 0, (), (:)) {
  let failed = try-parse((title: value), page)
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.map(problem => problem.code), ("schema.type",))
  assert.eq(failed.issues.map(problem => problem.path), (("title",),))
}
"#,
    );
}

#[test]
fn partial_parses_remaining_fields() {
    assert_schema(
        r#"
#let page = schema((title: str, tags: optional(array-of(str), default: ())))
#assert.eq(partial(page, tags: ("notes",))(title: "Notes"), (title: "Notes", tags: ("notes",)))
#assert.eq(partial(page)(title: "Notes"), parse((title: "Notes"), page))
"#,
    );
}

#[test]
fn partial_refuses_duplicate_field() {
    reject_schema(
        r#"
#let page = schema((title: str, layout: str))
#let note = partial(page, title: "Notes")
#note(title: "Other", layout: "page")
"#,
    );
}

#[test]
fn defaults_list_declared_fallbacks_only() {
    assert_schema(
        r#"
#let child = schema((label: optional(str)))
#let page = schema((title: str, tags: optional(array-of(str), default: ()), child: child))
#assert.eq(defaults(page), (tags: ()))
#assert.eq(defaults(child), (:))
"#,
    );
}

#[test]
fn nullable_bypasses_the_inner_schema() {
    assert_schema(
        r#"
#let never = map(any, _ => panic("unexpected nullable callback"))
#assert.eq(parse(none, nullable(never)), none)
#assert.eq(parse((:), schema((name: nullable(optional(str))))), (:))
#assert.eq(try-parse((:), schema((name: nullable(str)))).issues.first().code, "schema.missing")
"#,
    );
}

#[test]
fn trim_and_non_empty() {
    assert_schema(
        r#"
#for inner in (trim(str), map(str, _ => ""), map(str, _ => ())) {
  let failed = try-parse(" ", non-empty(inner))
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.first().code, "schema.non-empty")
}
#assert.eq(parse(" ", trim(non-empty(str))), "")

#for value in (none, auto, false, 0, (), (:)) {
  let failed = try-parse(value, trim(any))
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.first().code, "schema.type")
}
"#,
    );
}

#[test]
fn object_failures_follow_field_order() {
    assert_schema(
        r#"
#let failed = try-parse(
  (extra: true, second: 2, first: "bad"),
  schema((first: int, second: str)),
)
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (
  ("schema.type", ("first",)),
  ("schema.type", ("second",)),
  ("schema.unknown-key", ("extra",)),
))
"#,
    );
}

#[test]
fn keep_retains_unvalidated_fields() {
    assert_schema(
        r#"
#assert.eq(
  parse((name: " Name ", extra: none), schema((name: trim(str)), unknown: "keep")),
  (name: "Name", extra: none),
)
"#,
    );
}

#[test]
fn containers_collect_each_child_failure() {
    assert_schema(
        r#"
#let declaration = schema((rows: array-of(dictionary-of(int))))
#let failed = try-parse((rows: ((z: false, a: "wrong"), (only: none))), declaration)
#assert.eq(failed.issues.map(problem => problem.path), (
  ("rows", 0, "z"), ("rows", 0, "a"), ("rows", 1, "only"),
))
"#,
    );
}

#[test]
fn map_keeps_result_shaped_values() {
    assert_schema(
        r#"
#let payload = (ok: false, issues: ("ordinary content",), value: 7)
#assert.eq(parse(1, map(int, _ => payload)), payload)
#assert.eq(parse(1, convert(int, _ => ok(payload))), payload)
"#,
    );
}

#[test]
fn conversion_checks_its_output_once() {
    assert_schema(
        r#"
#let output = map(str, value => value + "!")
#assert.eq(parse(7, map(int, str, output: output)), "7!")
#assert.eq(parse(7, convert(int, value => ok(str(value)), output: output)), "7!")
#let problem = issue("Not convertible", code: "conversion", path: ("part",))
#assert.eq(try-parse(7, convert(int, _ => err(problem))), err(problem))
"#,
    );
}

#[test]
fn callback_protocol_errors_propagate() {
    for declaration in [
        "refine(int, _ => none, message: \"Rejected\")",
        "check(int, _ => none)",
        "check(int, _ => (\"not an issue\",))",
        "convert(int, _ => 1)",
        "convert(int, _ => (ok: true, value: 1, issues: ()))",
        "convert(int, _ => (ok: false, issues: ()))",
        "convert(int, _ => (ok: false, value: 1, issues: (issue(\"No\"),)))",
    ] {
        reject_schema(&format!("#let parsed = try-parse(1, {declaration})"));
    }
}

#[test]
fn union_preserves_programmer_errors() {
    reject_schema(
        r#"#let parsed = try-parse(1, union(map(int, _ => panic("branch failure")), any))"#,
    );
}

#[test]
fn union_selects_the_first_success() {
    assert_schema(
        r#"
#assert.eq(parse("raw", union(map(str, _ => "first"), map(str, _ => "second"))), "first")
#let choice = union(optional(str), optional(int, default: 7))
#assert.eq(parse((:), schema((value: choice))), (:))
#assert.eq(parse((value: 3), schema((value: choice))), (value: 3))
"#,
    );
}

#[test]
fn union_folds_absent_branches() {
    assert_schema(
        r#"
#let failed = try-parse((:), schema((value: union(str, int))))
#assert.eq(failed.issues.len(), 1)
#let problem = failed.issues.first()
#assert.eq((problem.code, problem.path), ("schema.missing", ("value",)))
#assert.eq((problem.notes, problem.children), ((), ()))
"#,
    );
}

#[test]
fn descriptions_override_same_node() {
    assert_schema(
        r#"
#let described = describe(optional(describe(int, "integer value")), "publication year")
#assert.eq(inspect(described).description, "publication year")
#assert.eq(try-parse("bad", described), try-parse("bad", optional(int)))
#assert.eq(
  try-parse((:), schema((value: union(describe(str, "Name"))))),
  try-parse((:), schema((value: union(str)))),
)
"#,
    );
}

#[test]
fn generated_values_report_their_origin() {
    assert_schema(
        r#"
#let cause-of(input, declaration) = try-parse(input, declaration).issues.first()
#let cases = (
  (
    cause: cause-of((:), schema((rows: optional(array-of(schema((count: int))), default: ((count: "bad"),))))),
    path: ("rows",),
    origin: ("default", (0, "count")),
    labeled: ("$.rows", "$[0].count"),
  ),
  (
    cause: cause-of((value: "raw"), schema((value: map(str, _ => (count: "bad"), output: schema((count: int)))))),
    path: ("value",),
    origin: ("output", ("count",)),
    labeled: ("$.value", "$.count"),
  ),
)
#for case in cases {
  let cause = case.cause
  assert.eq(cause.code, "schema.type")
  assert.eq(cause.path, case.path)
  assert.eq((cause.origin.kind, cause.origin.path), case.origin)
  assert.eq(cause.children, ())
  let rendered = format-issues((cause,))
  assert(rendered.contains(case.labeled.at(0)))
  assert(rendered.contains(case.labeled.at(1)))
}
"#,
    );
}

#[test]
fn relations_require_valid_fields() {
    assert_schema(
        r#"
#let fields = schema((title: str, start: map(int, value => value + 1), end: int, note: optional(str)))
#let relation = check(fields, values => {
  assert.eq(values, (start: 2, end: 1))
  (issue("End precedes start", code: "range", path: ("end",)),)
}, on: ("start", "end", "note"))
#let failed = try-parse((title: 7, start: 1, end: 1), relation)
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (
  ("schema.type", ("title",)), ("range", ("end",)),
))

#let fields = schema((start: int, end: int))
#let relation = check(fields, _ => panic("failed dependency reached callback"), on: ("start", "end"))
#let failed = try-parse((start: "bad", end: 2), relation)
#assert.eq(failed.issues.map(problem => problem.path), (("start",),))
"#,
    );
}

#[test]
fn explicit_none_fails_its_relation() {
    assert_schema(
        r#"
#let relation = check(nullable(schema((count: int))), _ => (), on: ("count",))
#let failed = try-parse(none, relation)
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.first().code, "schema.type")
#assert.eq(failed.issues.first().path, ())
"#,
    );
}

#[test]
fn relation_boundaries_are_declared() {
    for declaration in [
        "check(schema((count: int)), _ => (), on: (\"missing\",))",
        "check(map(schema((count: int)), value => value), _ => (), on: (\"count\",))",
        "check(convert(schema((count: int)), ok), _ => (), on: (\"count\",))",
        "check(union(schema((count: int)), dictionary), _ => (), on: (\"count\",))",
        "check(lazy(() => schema((count: int))), _ => (), on: (\"count\",))",
    ] {
        reject_schema(&format!("#let declaration = {declaration}"));
    }
}

#[test]
fn wrong_arity_reports_members() {
    assert_schema(
        r#"
#let declaration = tuple(optional(str), int)
#for values in ((none,), (none, "wrong", true)) {
  let failed = try-parse(values, declaration)
  assert.eq(failed.ok, false)
  assert(failed.issues.any(problem => problem.code == "schema.arity" and problem.path == ()))
  assert(failed.issues.any(problem => problem.code == "schema.type" and problem.path == (0,)))
  if values.len() > 1 {
    assert(failed.issues.any(problem => problem.code == "schema.type" and problem.path == (1,)))
  }
}
"#,
    );
}

#[test]
fn tuple_rest_applies_its_schema() {
    assert_schema(
        r#"
#let declaration = tuple(int, rest: trim(str))
#assert.eq(parse((1, " two ", " three "), declaration), (1, "two", "three"))
#let failed = try-parse((1, false, none), declaration)
#assert.eq(failed.issues.map(problem => problem.path), ((1,), (2,)))
#assert.eq(parse((), tuple()), ())
"#,
    );
}

#[test]
fn lazy_recursion_retains_child_paths() {
    assert_schema(
        r#"
#let node-schema() = schema((value: int, children: optional(array-of(lazy(node-schema)), default: ())))
#assert.eq(parse((value: 1, children: ((value: 2),)), node-schema()), (
  value: 1, children: ((value: 2, children: ()),),
))
#let failed = try-parse((value: 1, children: ((value: 2, children: ((value: "bad"),)),)), node-schema())
#assert.eq(failed.issues.map(problem => problem.path), (("children", 0, "children", 0, "value"),))
"#,
    );
}

#[test]
fn lazy_schema_waits_for_node() {
    assert_schema(
        r#"
#let deferred = lazy(() => panic("absent recursive node was evaluated"))
#assert.eq(parse((:), schema((node: optional(deferred)))), (:))
"#,
    );
}

#[test]
fn variant_runs_tagged_branch() {
    assert_schema(
        r#"
#let declaration = variant("kind",
  schema((kind: literal("note"), body: str)),
  schema((kind: literal("image"), width: map(int, _ => panic("inactive branch")))),
)
#assert.eq(parse((kind: "note", body: "Hello"), declaration), (kind: "note", body: "Hello"))
#for value in ((:), (kind: "unknown")) {
  let failed = try-parse(value, declaration)
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.map(problem => problem.path), (("kind",),))
}
"#,
    );
}

#[test]
fn malformed_declarations_are_errors() {
    for declaration in [
        "err()",
        "union()",
        "enum-of(())",
        "schema((count: value => true))",
        "schema((count: int), unknown: \"discard\")",
        "variant(\"kind\", schema((kind: optional(literal(\"note\")))))",
        "variant(\"kind\", schema((kind: map(literal(\"note\"), value => value))))",
        "variant(\"kind\", schema((kind: literal(\"note\"))), schema((kind: literal(\"note\"))))",
        "try-parse(1, lazy(() => \"not a schema\"))",
    ] {
        reject_schema(&format!("#let declaration = {declaration}"));
    }
}

#[test]
fn enum_membership_selects_declared_values() {
    assert_schema(
        r#"
#let kinds = enum-of(("draft", "published"))
#assert.eq(parse("draft", kinds), "draft")
#assert.eq(try-parse("archived", kinds).issues.map(problem => (problem.code, problem.path)), (
  ("schema.enum", ()),
))
#assert.eq(try-parse((:), schema((kind: kinds))).issues.first().code, "schema.missing")
"#,
    );
}

#[test]
fn one_or_many() {
    assert_schema(
        r#"
#assert.eq(parse(((1,), (2,)), one-or-many(array)), ((1,), (2,)))
#let failed = try-parse((1, 2), one-or-many(array))
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.map(problem => problem.path), ((0,), (1,)))

#let failed = try-parse((tags: 1), schema((tags: one-or-many(str))))
#assert.eq(failed.issues.map(problem => problem.path), (("tags",),))
#assert.eq(parse("tag", one-or-many(str)), ("tag",))
"#,
    );
}

#[test]
fn array_members_never_become_absent() {
    assert_schema(
        r#"
#let declaration = array-of(optional(str, default: "fallback"))
#let failed = try-parse(("present", none), declaration)
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (("schema.type", (1,)),))
#assert.eq(parse((), declaration), ())
"#,
    );
}

#[test]
fn length_counts_graphemes_and_members() {
    assert_schema(
        r#"
#let one = max-length(min-length(str, 1), 1)
#for value in ("e\u{301}", "\u{1100}\u{1161}") {
  assert.eq(parse(value, one), value)
}
#assert.eq(try-parse("", one).issues.first().code, "schema.min-length")
#assert.eq(try-parse("e\u{301}x", one).issues.first().code, "schema.max-length")

#let pair = max-length(min-length(any, 2), 2)
#for value in ((1, 2), (first: none, second: false)) {
  assert.eq(parse(value, pair), value)
}
#assert.eq(try-parse((1,), pair).issues.first().code, "schema.min-length")
#assert.eq(try-parse((a: 1, b: 2, c: 3), pair).issues.first().code, "schema.max-length")
#assert.eq(try-parse(2, pair).issues.first().code, "schema.type")
"#,
    );
}

#[test]
fn regex_rules_use_native_search() {
    assert_schema(
        r#"
#assert.eq(parse(" x ", matches(str, regex("x"))), " x ")
#assert.eq(try-parse(" x ", matches(str, regex("^x$"))).issues.first().code, "schema.matches")
#assert.eq(try-parse(7, matches(any, regex("7"))).issues.first().code, "schema.type")
"#,
    );
}

#[test]
fn numeric_bounds_are_inclusive() {
    assert_schema(
        r#"
#let bounded = at-most(at-least(any, 1), 2.5)
#for value in (1, 1.0, 2, 2.5) { assert.eq(parse(value, bounded), value) }
#assert.eq(try-parse(0.9, bounded).issues.first().code, "schema.at-least")
#assert.eq(try-parse(2.6, bounded).issues.first().code, "schema.at-most")
#for value in (true, "2", 2pt, 20%, none) {
  assert.eq(try-parse(value, bounded).issues.first().code, "schema.type")
}
"#,
    );
}
