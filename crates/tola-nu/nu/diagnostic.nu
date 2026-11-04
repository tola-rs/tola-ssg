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

# Submit a complete report to the host, including an empty list when there are no issues.
# Example: `[(diagnostic issue "Check the title" --code title)] | diagnostic submit`
export def submit [
  --path: path # Override the host's TOLA_REPORT_PATH.
]: list<record> -> nothing {
  let issues = $in
  let destination = ($path | default ($env.TOLA_REPORT_PATH?))
  if $destination == null {
    error make {msg: "TOLA_REPORT_PATH is not set; pass --path to write a report"}
  }
  {schemaVersion: 1, diagnostics: $issues} | to json | save --raw --force $destination
}
