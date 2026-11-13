# Public diagnostic library and JSON rendering entry.
export use ./diagnostic_core.nu [make submit render]

# Render diagnostics from JSON on stdin.
export def main [] {
  help main
}

# Read a JSON list or an object with a diagnostics list and print the diagnostics.
def "main render" [] {
  let input = ($in | from json --strict)
  let diagnostics = match $input {
    {diagnostics: $items} => $items
    _ => $input
  }
  $diagnostics | render
}
