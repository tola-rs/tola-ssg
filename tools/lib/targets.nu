# Every target a release publishes, in the order a host target is chosen.

export const TARGETS = {
  'x86_64-unknown-linux-musl': { system: 'linux', machine: 'x86_64', binary: 'tola', linkage: 'static' }
  'aarch64-unknown-linux-musl': { system: 'linux', machine: 'aarch64', binary: 'tola', linkage: 'static' }
  'x86_64-unknown-linux-gnu': { system: 'linux', machine: 'x86_64', binary: 'tola', linkage: 'dynamic' }
  'aarch64-unknown-linux-gnu': { system: 'linux', machine: 'aarch64', binary: 'tola', linkage: 'dynamic' }
  'aarch64-apple-darwin': { system: 'darwin', machine: 'aarch64', binary: 'tola', linkage: 'dynamic' }
  'x86_64-pc-windows-msvc': { system: 'windows', machine: 'x86_64', binary: 'tola.exe', linkage: 'dynamic' }
}

export def target-names []: nothing -> list<string> {
  $TARGETS | columns
}

export def target [name: string]: nothing -> record {
  if not ($name in $TARGETS) {
    error make { msg: $"unknown target: '($name)' (choose from (target-names | str join ', '))" }
  }
  $TARGETS | get $name
}

# The versioned, conventionally named archive a target publishes.
export def archive-name [name: string, version: string]: nothing -> string {
  let kind = (target $name | get system)
  let extension = if $kind == 'windows' { 'zip' } else { 'tar.gz' }
  $"tola-($version)-($name).($extension)"
}

export def license-archive-name [version: string]: nothing -> string {
  $"tola-($version)-licenses.tar.gz"
}

export def matches-host [entry: record]: nothing -> bool {
  let host = $nu.os-info
  let system = if $host.name == 'macos' { 'darwin' } else { $host.name }
  $entry.system == $system and $entry.machine == $host.arch
}

# The default target this host builds.
export def host-target []: nothing -> string {
  let name = (target-names | where {|name| matches-host (target $name) } | first)
  if $name == null {
    let host = $nu.os-info
    error make { msg: $"no release target is built on this host (($host.name)/($host.arch))" }
  }
  $name
}
