# Construct an issue for a Tola diagnostic report.
# Omit locations in Nu hooks and module commands. Located reports need a host source snapshot.
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

# Print issues for an independent check. Locations use a file path.
# Use submit inside a hook or module command to include issues in the host's result.
export def render []: list<record> -> nothing {
  let executable = ($env.TOLA_EXECUTABLE? | default "tola")
  $in | to json | ^$executable diagnostic render
}
