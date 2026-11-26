#show: package(
  summary: [Emit custom errors, warnings, info messages, and hints during Typst evaluation.],
  related: ("@tola-scripts/diagnostic", "[diagnostics]"),
)

#export(
  "report",
  summary: [Record a site diagnostic without immediately aborting Typst evaluation.],
  params: (
    message: [Human-readable explanation of the issue.],
    severity: [Diagnostic level: `"error"`, `"warning"` (default), `"info"`, or `"hint"`.],
    code: [Optional rule identifier owned by your site or module (for example `"missing-summary"`).],
    at: [Location anchor: `auto` (default) points to the `report(...)` call, Typst content points to that element's source span, and `none` omits location.],
    hints: [Optional array of short fix suggestions shown below the message.],
    related: [Optional array of `(message: str, at: content | none)` records pointing to related spans.],
  ),
)[
  Any `"error"` diagnostic fails `tola check` and `tola build` after evaluation finishes, while `"warning"`, `"info"`, and `"hint"` allow the build to succeed. Use Typst's `panic(...)` only when evaluation cannot continue.

  #example(title: [Warn when an article omits a description], check: "default")[
  ```typst
  #import "@tola/diagnostic:0.0.0": report

  #let check-page(title, description) = {
    if description == none {
      report(
        "Page \"" + title + "\" is missing a description.",
        severity: "warning",
        code: "site.missing-description",
        hints: ("Add `description` in `tola-meta`.",),
      )
    }
  }
  ```
  ]
]
