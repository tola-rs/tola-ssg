use tola-native render-diagnostics
export use tola-native submit

# Construct a Tola diagnostic record.
export def make [
  message: string
  --severity: string = "warning" # error, warning, info, or hint.
  --code: string
  --hints: list<string> = []
  --notes: list<string> = []
  --related: list<record> = []
]: nothing -> record {
  if $severity not-in [error warning info hint] {
    error make {msg: "severity must be error, warning, info, or hint"}
  }
  {
    severity: $severity
    message: $message
    code: $code
    hints: (if not ($hints | is-empty) { $hints })
    notes: (if not ($notes | is-empty) { $notes })
    related: (if not ($related | is-empty) { $related })
  } | compact
}

# Print diagnostics for an independent check. Locations use file paths.
export def render []: list<record> -> nothing {
  render-diagnostics | print --no-newline
}
