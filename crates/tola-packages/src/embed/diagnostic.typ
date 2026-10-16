// @tola/diagnostic:0.0.0 - structured reports from code already running in Typst

/// Report an issue without stopping Typst evaluation.
///
/// The default location is this function's actual call. Pass content as `at` to use that
/// content's native source span, or `none` for an issue without a source location. Generated
/// or transformed content can have no span; no source location is guessed in that case.
/// A layout location, string path, or dictionary cannot stand in for source content.
///
/// Tola collects error, warning, info, and hint reports. An error prevents a successful check
/// or publication after evaluation; use Typst's native error path to stop evaluation immediately.
/// The calling project or package owns the report and its optional rule code.
///
/// Example - report about authored content, add a hint, and relate another source value:
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
/// - severity (str): `"error"`, `"warning"`, `"info"`, or `"hint"`; defaults to `"warning"`.
/// - code (str | none): An optional nonempty rule code owned by the caller.
/// - at (auto | content | none): The call, a content value's native span, or no location.
/// - hints (array): Explanatory strings.
/// - related (array): Dictionaries with exactly `message` (str) and `at` (content or none).
/// -> none
#import "@tola/host:0.0.0": report
