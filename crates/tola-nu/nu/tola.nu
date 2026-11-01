use tola/cli/cli.nu capture

# Evaluate the CLI's query without changing the target's module or configuration identity.
export def query [
  target: string
  --config: path # Overrides the host's selected configuration; otherwise use normal CLI discovery.
  --label: string # Metadata label for an explicit Typst query file.
  --report # Return the complete query report instead of its data.
]: nothing -> any {
  mut argv = [query]
  let configuration = $config | default $env.TOLA_CONFIG_PATH?
  if $configuration != null { $argv = ($argv | append [--config $configuration]) }
  if $label != null { $argv = ($argv | append [--label $label]) }
  if $report { $argv = ($argv | append [--report]) }
  let reply = capture ($argv | append [-- $target])
  if $reply.stderr != '' { print --stderr --no-newline $reply.stderr }
  if $reply.exit_code != 0 {
    error make {msg: $'Tola query failed with exit code ($reply.exit_code)'}
  }
  $reply.stdout | from json
}
