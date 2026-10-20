#import "@tola/schema:0.0.0": map, nullable, trim

/// Treat an empty string as absent.
///
/// An empty attribute would claim a value that is not there, so the web helpers write no entry
/// for it.
#let empty-to-none(value) = if value == "" { none } else { value }

/// A schema for one optional text option: the string is trimmed, and an empty one counts as absent.
#let optional-text = map(
  nullable(trim(str)),
  empty-to-none,
  output: nullable(str),
)
