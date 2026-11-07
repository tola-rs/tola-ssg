#!/usr/bin/env nu
# Check and update the synchronized Rust workspace without publishing.

use ../lib/repo.nu *
use ../lib/semver.nu *

const USAGE = 'usage: nu tools/release/version.nu preview VERSION
       nu tools/release/version.nu set VERSION
       nu tools/release/version.nu check [--tag TAG] [--commit COMMIT]'

# Rewrite the two places the release version is written, leaving every other byte alone.
def rewrite-manifest [source: string, version: string, members: list<string>]: nothing -> string {
  mut lines = []
  mut section = ''
  for line in ($source | lines) {
    let heading = ($line | parse --regex '^\[(?<name>[^\]]+)\]$' | first)
    if not ($heading | is-empty) {
      $section = $heading.name
      $lines = ($lines | append $line)
      continue
    }
    if $section == 'workspace.package' and ($line | parse --regex '^\s*(?<key>version)\s*=' | is-not-empty) {
      $lines = ($lines | append $"version = \"($version)\"")
      continue
    }
    if $section == 'workspace.dependencies' {
      let dependency = ($line | parse --regex '^(?<alias>tola-[A-Za-z0-9_-]+)\s*=\s*\{' | first)
      if (not ($dependency | is-empty)) and ($dependency.alias in $members) {
        $lines = ($lines | append ($line | str replace --regex 'version\s*=\s*"[^"]*"' $"version = \"($version)\""))
        continue
      }
    }
    $lines = ($lines | append $line)
  }
  $lines | str join (char newline)
}

# Print only the lines a preview would change.
def show-change [before: string, after: string] {
  if ($before | lines) == ($after | lines) {
    print 'Workspace already uses the requested version; no changes.'
    return
  }
  let old = ($before | lines)
  let new = ($after | lines)
  for index in 0..<($old | length) {
    let current = ($old | get $index)
    let replacement = ($new | get $index)
    if $current != $replacement {
      print $"- ($current)"
      print $"+ ($replacement)"
    }
  }
}

def plan [root: string, version: string]: nothing -> record {
  let current = (workspace-version $root)
  parse-version $version | ignore
  if (compare-versions (parse-version $version) (parse-version $current)) < 0 {
    error make { msg: $"refusing version downgrade from ($current) to ($version)" }
  }
  let manifest = (open ($root | path join 'Cargo.toml'))
  let source = (open --raw ($root | path join 'Cargo.toml'))
  # Every internal dependency is named tola-*; a preview must not need Cargo.
  let members = ($manifest | get --optional workspace | default {}
    | get --optional dependencies | default {} | columns | where { |name| $name | str starts-with 'tola-' })
  { source: $source, rewritten: (rewrite-manifest $source $version $members) }
}

# Apply the version and let Cargo own lockfile resolution.
def apply [root: string, version: string] {
  require-clean $root
  let prepared = (plan $root $version)
  if ($prepared.source | lines) == ($prepared.rewritten | lines) {
    print 'Workspace already uses the requested version; no changes.'
    return
  }
  let manifest = ($root | path join 'Cargo.toml')
  let lockfile = ($root | path join 'Cargo.lock')
  let original_lock = (open --raw $lockfile)
  let before = (lock-external $root)
  $prepared.rewritten | save --force $manifest
  let result = (^cargo metadata --offline --all-features --format-version 1
    --manifest-path $manifest | complete)
  let restore = {
    $prepared.source | save --force $manifest
    $original_lock | save --force $lockfile
  }
  if $result.exit_code != 0 {
    do $restore
    error make { msg: $"cargo metadata --offline failed; nothing was applied: ($result.stderr | str trim)" }
  }
  if ($before | to json) != ((lock-external $root) | to json) {
    do $restore
    error make { msg: 'version update would change third-party dependency resolution; nothing was applied' }
  }
  check-version $root | ignore
  show-change $prepared.source $prepared.rewritten
  print $"Workspace version ($version) prepared. No commit, tag, push, or publication was performed."
}

def main [
  command: string
  version?: string
  --tag: string = ''
  --commit: string = ''
] {
  let root = ($env.FILE_PWD | path dirname | path dirname)
  match $command {
    'preview' => {
      if $version == null { error make { msg: 'preview needs a version' } }
      let prepared = (plan $root $version)
      show-change $prepared.source $prepared.rewritten
    }
    'set' => {
      if $version == null { error make { msg: 'set needs a version' } }
      apply $root $version
    }
    'check' => {
      if $commit != '' and $tag == '' { error make { msg: '--commit requires --tag' } }
      if $tag != '' { check-tag $root $tag ($commit | if $in == '' { null } else { $in }) | ignore }
      let checked = (check-version $root)
      print $"Workspace version ($checked.version): ($checked.packages | length) packages, internal dependencies, and Cargo.lock agree."
    }
    _ => {
      error make { msg: $"unknown command: ($command)\n\n($USAGE)" }
    }
  }
}
