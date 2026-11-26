#show: package(
  summary: [Declare, compose, inspect, and run validation schemas over metadata and Typst values.],
  related: ("@tola/source#parse-sources",),
)

Pass a dictionary of field rules to `schema(...)`. Each field accepts a Typst type (such as `str` or `int`) or a schema rule (such as `array-of(str)`). Wrappers evaluate from the inside out: `non-empty(trim(str))` trims whitespace first, then rejects empty strings.

Use `parse` to return the validated value or raise an error; use `try-parse` to handle `(ok:, value:)` / `(ok:, issues:)` directly. For content file metadata, prefer #ref("@tola/source#parse-sources") so errors point to the source file.

#example(title: [Validate page metadata with defaults and constraints], check: "default")[
```typst
#import "@tola/schema:0.0.0": array-of, date, non-empty, optional, parse, schema, trim

#let post-schema = schema((
  title: non-empty(trim(str)),
  draft: optional(bool, default: false),
  published: optional(date),
  tags: optional(array-of(non-empty(trim(str))), default: ()),
))

#let meta = parse((title: "  Notes  "), post-schema)
```
]

#export(
  "any",
  summary: [Schema accepting any Typst value unchanged.],
)

#export(
  "schema",
  summary: [Declare a dictionary schema mapping field names to types or child schemas.],
  params: (
    fields: [Dictionary mapping field names to Typst types or `@tola/schema` rules.],
    unknown: [How undeclared keys are handled: `"error"` (default, reject), `"strip"` (drop), or `"keep"` (preserve unchanged).],
  ),
  related: ("optional", "nullable", "partial", "defaults", "parse"),
)

#export(
  "optional",
  summary: [Allow a dictionary field to be omitted, optionally supplying a default value when absent.],
  params: (
    member: [Inner type or schema validated whenever the key is present.],
    options: [Optional named `default: value` used when the key is missing (must itself satisfy `member`). Invalid supplied values fail rather than falling back to `default`.],
  ),
  related: ("nullable", "schema", "defaults"),
)

#export(
  "nullable",
  summary: [Allow `none` in addition to values matching `member`.],
  params: (
    member: [Inner type or schema checked when the value is not `none`.],
  ),
  related: ("optional", "union"),
)

#export(
  "literal",
  summary: [Match only a single exact Typst value.],
  params: (
    value: [Exact expected value.],
  ),
  related: ("enum-of", "variant"),
)

#export(
  "enum-of",
  summary: [Match any value equal to one of the elements in `values`.],
  params: (
    values: [Non-empty array of allowed values.],
  ),
  related: ("literal", "union"),
)

#export(
  "array-of",
  summary: [Validate an array whose elements each satisfy `member`.],
  params: (
    member: [Element type or schema.],
  ),
  related: ("one-or-many", "tuple", "min-length", "max-length"),
)

#export(
  "dictionary-of",
  summary: [Validate a dictionary whose values each satisfy `member`.],
  params: (
    member: [Value type or schema applied to every entry.],
  ),
  related: ("schema",),
)

#export(
  "one-or-many",
  summary: [Accept either a single `member` value or an array of `member` values, normalizing to an array.],
  params: (
    member: [Element type or schema.],
  ),
  related: ("array-of",),
)

#export(
  "tuple",
  summary: [Validate a fixed-position array with per-slot schemas and an optional trailing `rest` schema.],
  params: (
    members: [Positional schemas for each required index.],
    rest: [Optional schema for extra elements after `members`; `none` (default) rejects extra elements.],
  ),
  related: ("array-of",),
)

#export(
  "lazy",
  summary: [Defer schema construction to support recursive data structures.],
  params: (
    factory: [Zero-argument function `() => schema` evaluated on demand.],
  ),
)

#export(
  "union",
  summary: [Try candidate schemas in order and accept the first match.],
  params: (
    members: [Two or more candidate types or schemas tried from left to right.],
  ),
  related: ("variant", "enum-of"),
)

#export(
  "variant",
  summary: [Select a branch schema by matching a discriminator field (`tag-key`).],
  params: (
    tag-key: [Field name holding the discriminator value.],
    branches: [Candidate `schema(...)` declarations, each fixing `tag-key` with `literal(...)` or `enum-of(...)`.],
  ),
  related: ("union", "literal", "schema"),
)

#export(
  "trim",
  summary: [Strip leading and trailing whitespace from a string before passing it to `member`.],
  params: (
    member: [Inner string schema (typically `str`).],
  ),
  related: ("non-empty",),
)

#export(
  "non-empty",
  summary: [Reject empty strings, arrays, dictionaries, or bytes after validating `member`.],
  params: (
    member: [Inner type or schema.],
  ),
  related: ("trim", "min-length"),
)

#export(
  "refine",
  summary: [Add a boolean predicate check to `member`.],
  params: (
    member: [Inner type or schema run first.],
    predicate: [Function `(value) => bool` returning `true` when valid.],
    message: [Optional error message when `predicate` returns `false`.],
  ),
  related: ("check", "map", "convert"),
)

#export(
  "check",
  summary: [Attach custom multi-issue validation logic to `member`.],
  params: (
    member: [Inner type or schema run first.],
    inspect: [Function `(value) => array` returning zero or more `issue(...)` records.],
    on: [Optional field path or key where reported issues attach.],
  ),
  related: ("refine", "issue"),
)

#export(
  "map",
  summary: [Transform a validated value with `convert-value` (cannot fail with schema issues).],
  params: (
    member: [Inner type or schema run first.],
    convert-value: [Function `(value) => new-value` applied after `member` succeeds.],
    output: [Optional schema describing the mapped output type for `inspect`.],
  ),
  related: ("convert", "refine"),
)

#export(
  "convert",
  summary: [Validate `member`, then transform it with a fallible converter returning `ok(...)` or `err(...)`.],
  params: (
    member: [Inner type or schema run first.],
    convert-value: [Function `(value) => ok(v) | err(...)`.],
    output: [Optional schema describing the converted output type for `inspect`.],
  ),
  related: ("ok", "err", "issue", "map"),
)

#export(
  "describe",
  summary: [Attach a human-readable description string to `member` for `inspect`.],
  params: (
    member: [Inner type or schema.],
    description: [Non-empty description string.],
  ),
  related: ("inspect",),
)

#export(
  "inspect",
  summary: [Return a structured description of a schema's fields, types, defaults, and descriptions without parsing data.],
  params: (
    member: [Schema to inspect.],
    input: [When `true`, describe the accepted input shape before conversions; `false` (default) describes the output shape.],
  ),
  related: ("describe", "defaults"),
)

#export(
  "parse",
  summary: [Validate `value` against `member` and return the parsed result, raising a formatted error on failure.],
  params: (
    value: [Input value to validate.],
    member: [Type or schema declaration.],
  ),
  related: ("try-parse", "format-issues", "@tola/source#parse-sources"),
)

#export(
  "try-parse",
  summary: [Validate `value` against `member` without panicking, returning `(ok: true, value: ...)` or `(ok: false, issues: ...)` .],
  params: (
    value: [Input value to validate.],
    member: [Type or schema declaration.],
  ),
  related: ("parse", "format-issues"),
)

#export(
  "partial",
  summary: [Make all fields of an object `schema` optional, except any field names listed in `fixed`.],
  params: (
    member: [Object schema created with `schema(...)`.],
    fixed: [Optional field names that remain required.],
  ),
  related: ("schema", "optional"),
)

#export(
  "defaults",
  summary: [Materialize the default dictionary defined by an object `schema` (requires every field to be optional or have a default).],
  params: (
    member: [Object schema whose defaults to resolve.],
  ),
  related: ("schema", "optional", "inspect"),
)

#export(
  "ok",
  summary: [Wrap a successful converted value returned from a `convert` callback.],
  params: (
    value: [Converted value.],
  ),
  related: ("err", "convert"),
)

#export(
  "err",
  summary: [Wrap one or more validation issues returned from a `convert` callback.],
  params: (
    issues: [One or more `issue(...)` records (or message strings).],
  ),
  related: ("ok", "issue", "convert"),
)

#export(
  "issue",
  summary: [Construct a structured validation issue for `check` or `convert`.],
  params: (
    message: [Human-readable problem description.],
    code: [Optional machine-readable issue code.],
    path: [Optional array of path segments locating the offending field or index.],
    notes: [Optional array of explanatory note strings.],
  ),
  related: ("err", "check", "format-issues"),
)

#export(
  "format-issues",
  summary: [Format an array of issues from `try-parse` into readable multi-line text.],
  params: (
    issues: [Array of issue records.],
  ),
  related: ("try-parse", "issue"),
)

#export(
  "min-length",
  summary: [Require a string, array, dictionary, or bytes value to have length at least `minimum`.],
  params: (
    member: [Inner type or schema.],
    minimum: [Non-negative minimum length.],
  ),
  related: ("max-length", "non-empty"),
)

#export(
  "max-length",
  summary: [Require a string, array, dictionary, or bytes value to have length at most `maximum`.],
  params: (
    member: [Inner type or schema.],
    maximum: [Non-negative maximum length.],
  ),
  related: ("min-length",),
)

#export(
  "matches",
  summary: [Require a string value to match a regular expression.],
  params: (
    member: [Inner string schema.],
    expression: [Typst `regex(...)` or pattern string.],
  ),
)

#export(
  "at-least",
  summary: [Require a numeric value (`int` or finite `float`) to be greater than or equal to `minimum`.],
  params: (
    member: [Inner numeric schema.],
    minimum: [Inclusive lower bound.],
  ),
  related: ("at-most",),
)

#export(
  "at-most",
  summary: [Require a numeric value (`int` or finite `float`) to be less than or equal to `maximum`.],
  params: (
    member: [Inner numeric schema.],
    maximum: [Inclusive upper bound.],
  ),
  related: ("at-least",),
)

#export(
  "email",
  summary: [Built-in string schema validating email address syntax.],
)

#export(
  "ipv4",
  summary: [Built-in string schema validating dotted-quad IPv4 addresses.],
  related: ("ipv6", "ip"),
)

#export(
  "ipv6",
  summary: [Built-in string schema validating IPv6 addresses (full or `::`-compressed).],
  related: ("ipv4", "ip"),
)

#export(
  "ip",
  summary: [Built-in string schema accepting either IPv4 or IPv6 addresses.],
  related: ("ipv4", "ipv6"),
)

#export(
  "http-url",
  summary: [Built-in string schema validating absolute `http://` or `https://` URLs.],
  related: ("https-url", "url-path"),
)

#export(
  "https-url",
  summary: [Built-in string schema validating absolute `https://` URLs.],
  related: ("http-url",),
)

#export(
  "url-path",
  summary: [Built-in schema validating a site-root URL path (such as a permalink) and decoding it for `output-of` or `route-url`.],
  related: ("@tola/address#output-of", "@tola/address#decode-url-path"),
)

#export(
  "date",
  summary: [Built-in schema parsing a calendar date (`"YYYY-MM-DD"` or Typst `datetime`) into a Typst `datetime` date.],
  related: ("partial-date", "timestamp"),
)

#export(
  "partial-date",
  summary: [Built-in schema parsing a year (`"YYYY"`), year-month (`"YYYY-MM"`), or full date into `(year:, month:, day:, precision:, iso:)`.],
  related: ("date", "timestamp"),
)

#export(
  "timestamp",
  summary: [Built-in schema parsing an RFC 3339 timestamp string (with timezone offset) into `(datetime:, offset:, rfc3339:)`.],
  related: ("date", "@tola/feed#feed", "@tola/sitemap#sitemap"),
)[
  Pass `.rfc3339` directly to #ref("@tola/feed#feed") or #ref("@tola/sitemap#sitemap").
]
