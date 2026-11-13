// @tola/schema:0.0.0 - validate metadata and other Typst values
//
// Give `schema` a dictionary of field declarations. Each field takes a Typst type such as `str`
// or a declaration such as `array-of(str)`. Use `optional` to allow a missing key and `nullable`
// to allow `none`.
//
// Use `parse` to get the result or raise an error if the value fails validation. With `try-parse`
// you can handle the returned `issues` yourself or display them with `format-issues`.
//
// Wrappers run from the inside out. `non-empty(trim(str))` removes leading and trailing
// whitespace first, then rejects an empty string. Use `union` to try alternatives in order and
// `variant` to select an alternative by a field's value. Use `inspect` to look up fields,
// types, and defaults without parsing a value.
//
// For source metadata, use `parse-sources` from `@tola/source` in your `site.typ`.
#import "requirements.typ": (
  any, array-of, at-least, at-most, check, convert, date, defaults, describe, dictionary-of,
  email, enum-of, err, format-issues, http-url, https-url, inspect, ip, ipv4, ipv6, issue, lazy, literal,
  map, matches, max-length, min-length, non-empty, nullable, ok, one-or-many, optional, parse, partial,
  partial-date, refine, schema, timestamp, trim, try-parse, tuple, union, url-path, variant,
)
