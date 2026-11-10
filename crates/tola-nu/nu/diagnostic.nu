# Construct an issue for a Tola diagnostic report.
# A location refers to a source ID and text snapshot supplied by the host.
export def issue [
  message: string
  --severity: string = "warning" # error, warning, info, or hint.
  --code: string
  --location: record # Host source ID and UTF-16 range, when supplied by the host.
  --hints: list<string> = []
  --notes: list<string> = []
  --related: list<record> = []
]: nothing -> record {
  if $severity not-in [error warning info hint] {
    error make {msg: "severity must be error, warning, info, or hint"}
  }
  mut issue = {severity: $severity, message: $message}
  if $code != null { $issue = ($issue | insert code $code) }
  if $location != null { $issue = ($issue | insert location $location) }
  if not ($hints | is-empty) { $issue = ($issue | insert hints $hints) }
  if not ($notes | is-empty) { $issue = ($issue | insert notes $notes) }
  if not ($related | is-empty) { $issue = ($issue | insert related $related) }
  $issue
}

# Render issues through the same Rust renderer the build uses.
export def render []: list<record> -> nothing {
  let executable = ($env.TOLA_EXECUTABLE? | default "tola")
  $in | to json | ^$executable diagnostic render
}

# Submit diagnostics to the host.
export def submit []: list<record> -> nothing {
  $in | __tola_diagnostic_submit
}
