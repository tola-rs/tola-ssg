use ./context_core.nu get-context

# Show the context Tola supplied to the current hook, module command, or initializer.
export def main [
  --json # Print JSON for use by external programs.
] {
  get-context --json=$json
}
