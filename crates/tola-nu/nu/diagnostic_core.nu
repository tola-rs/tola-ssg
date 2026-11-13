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
  mut issue = {severity: $severity, message: $message}
  if $code != null { $issue = ($issue | insert code $code) }
  if not ($hints | is-empty) { $issue = ($issue | insert hints $hints) }
  if not ($notes | is-empty) { $issue = ($issue | insert notes $notes) }
  if not ($related | is-empty) { $issue = ($issue | insert related $related) }
  $issue
}

# Print diagnostics for an independent check. Locations use file paths.
export def render []: list<record> -> nothing {
  print --no-newline ($in | render-diagnostics)
}
