// @tola/schema:0.0.0 - declare the shape of a value, then parse values into it or collect issues
//
// A declaration describes a value, and parsing checks a value against it. A declaration is a
// reusable Typst value: you can name it, store it, and nest it inside another declaration.
//
// Start with `schema`. It names each field and gives every field a declaration, either a Typst type
// such as `str` or one built from the combinators here. Add `optional` for a key that may be
// missing, and `nullable` for a key that may be `none`.
//
// Fill in the inner shapes with `array-of`, `dictionary-of`, `tuple`, `one-or-many`, `enum-of`, and
// `literal`. Use `union` to try several branches in order, and `variant` to pick one branch by a
// tag value.
//
// Nesting is ordinary function application, and the inner declaration decides first:
// `optional(array-of(trim(str)))` accepts a missing key, or an array whose members are trimmed.
//
// Hand a value and a declaration to `parse` to get the normalized value, or to `try-parse` to get a
// Result. The value you pass is never changed; a normalized value is a new value.
//
// `partial` fixes some fields for a later `parse`, and `defaults` reports the declared fallbacks.
//
// Wrap a declaration to change how its value is checked or shaped. `trim` and `non-empty`
// normalize and reject, `refine` and `check` validate against a callback, `map` transforms, and
// `convert` transforms with a possible rejection. Wrappers run from the inside out:
// `non-empty(trim(str))` trims first, then rejects an empty string.
//
// Failures are Issues; render them with `format-issues`. `describe` documents a value, and
// `inspect` derives the output shape from the declaration alone, without running a callback. A
// malformed declaration or a callback programming error stops compilation, while a failing value
// becomes an Issue. Callbacks may run again when an enclosing Typst context is reevaluated, so
// every parse starts fresh.
//
// Parse where a value enters: the root program parses all source metadata up front, and
// selection and templates then read the parsed fields without checking them again.
//
// Declarations and parsing work on values alone. For source metadata, use `@tola/source`'s
// `parse-sources`.

#import "requirements.typ": (
  any, array-of, at-least, at-most, check, convert, date, defaults, describe, dictionary-of,
  email, enum-of, err, format-issues, http-url, https-url, inspect, ip, ipv4, ipv6, issue, lazy, literal,
  map, matches, max-length, min-length, non-empty, nullable, ok, one-or-many, optional, parse, partial,
  refine, schema, timestamp, trim, try-parse, tuple, union, url-path, variant,
)
