// @tola/diagnostic:0.0.0 - report warnings and errors from Typst

/// Report an issue without stopping Typst evaluation.
///
/// To point to a specific heading or link, pass that content as `at`.
/// Generated content may have no source location.
///
/// An `error` fails the check or build. A `warning` highlights a problem while letting it
/// finish. To stop Typst immediately, use `panic`. Use `code` to name your rule.
///
///
/// Add this to `content/index.typ` in a site created with `tola init`. Run `tola check` to
/// see a warning at `value`, with a hint and a reference to `original`.
///
/// ```typst
/// #import "@tola/diagnostic:0.0.0": report
/// #let value = [Sample]
/// #let original = [Earlier sample]
/// #report("Review this value.", severity: "warning", code: "review-value", at: value,
///   hints: ("Choose a value supported by this module.",),
///   related: ((message: "Earlier value.", at: original),))
/// ```
///
/// - message (str): The explanation of the issue.
/// - severity (str): Use `"warning"` if omitted. Other choices are `"error"`, `"info"`, and `"hint"`.
/// - code (str | none): An optional nonempty name for the rule.
/// - at (auto | content | none): If omitted or `auto`, point to the call. Pass content to point to its source,
///   or `none` to omit the location.
/// - hints (array): Optional hints for the site author, such as ways to fix the issue.
/// - related (array): Dictionaries with exactly `message` (str) and `at` (content or none).
/// -> none
#import "@tola/host:0.0.0": report
