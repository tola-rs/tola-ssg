# Repository state and workspace version agreement.

use ./semver.nu *

export const MANIFEST = 'Cargo.toml'
export const LOCKFILE = 'Cargo.lock'

# Run git with a fixed argument list and fail with its stderr when it exits non-zero.
export def git-output [root: string, args: list<string>]: nothing -> string {
  let result = (^git -C $root ...$args | complete)
  if $result.exit_code != 0 {
    error make { msg: $"git ($args | str join ' ') failed: ($result.stderr | str trim)" }
  }
  $result.stdout | str trim
}

# The one version the workspace publishes.
export def workspace-version [root: string]: nothing -> string {
  let manifest = (open ($root | path join $MANIFEST))
  let package = ($manifest | get --optional workspace | default {} | get --optional package | default {})
  let version = ($package | get --optional version | default '')
  if $version == '' {
    error make { msg: 'Cargo.toml must define [workspace.package].version as the single release version' }
  }
  parse-version $version | ignore
  $version
}

# A version update and a release both start from a clean tree.
export def require-clean [root: string] {
  let status = (git-output $root ['status' '--porcelain=v1' '--untracked-files=all'])
  if $status != '' {
    error make { msg: 'working tree is dirty; commit or preserve your changes before updating a version or checking a release' }
  }
}

# The checked commit: the tag names the workspace version and points at HEAD.
export def check-tag [root: string, tag: string, commit?: string]: nothing -> string {
  let expected = (tag-version $tag)
  git-output $root ['check-ref-format' $"refs/tags/($tag)"] | ignore
  let actual = (workspace-version $root)
  if $expected != $actual {
    error make { msg: $"tag ($tag) does not match workspace version ($actual)" }
  }
  let head = (git-output $root ['rev-parse' '--verify' 'HEAD^{commit}'])
  if $commit != null {
    if ($commit | parse --regex '^(?<sha>[0-9a-fA-F]{40})$' | is-empty) {
      error make { msg: 'release commit must be a full 40-character Git object ID' }
    }
    let selected = (git-output $root ['rev-parse' '--verify' $"($commit)^{commit}"])
    if $selected != $head {
      error make { msg: $"checkout ($head) differs from release commit ($selected)" }
    }
  }
  let branch = (git-output $root ['for-each-ref' '--format=%(refname)' $"refs/heads/($tag)"])
  if $branch == $"refs/heads/($tag)" {
    error make { msg: $"release tag conflicts with a branch: ($tag)" }
  }
  let existing = (git-output $root ['for-each-ref' '--format=%(refname)' $"refs/tags/($tag)"])
  if $existing == $"refs/tags/($tag)" {
    let tagged = (git-output $root ['rev-parse' '--verify' $"refs/tags/($tag)^{commit}"])
    if $tagged != $head {
      error make { msg: $"tag ($tag) points to ($tagged), not checked commit ($head)" }
    }
  }
  $head
}

# Workspace members as Cargo reports them, without resolving third-party dependencies.
export def workspace-packages [root: string]: nothing -> list<record> {
  let result = (^cargo metadata --locked --no-deps --format-version 1
    --manifest-path ($root | path join $MANIFEST) | complete)
  if $result.exit_code != 0 {
    error make { msg: $"cargo metadata --locked failed: ($result.stderr | str trim)" }
  }
  $result.stdout | from json | get packages
}

def lock-packages [root: string]: nothing -> list<record> {
  open --raw ($root | path join $LOCKFILE) | from toml | get package
}

# Every member, internal dependency, and workspace entry in the lock agree on one version.
export def check-version [root: string]: nothing -> record {
  let version = (workspace-version $root)
  let packages = (workspace-packages $root)
  let manifest = (open ($root | path join $MANIFEST))
  let dependencies = ($manifest | get --optional workspace | default {}
    | get --optional dependencies | default {})
  let locked = (lock-packages $root | where {|entry| $entry.source? == null })
  for package in $packages {
    let relative = ($package.manifest_path | path relative-to $root)
    if $package.version != $version {
      error make { msg: $"($relative): effective version ($package.version) differs from ($version)" }
    }
    let declared = (open $package.manifest_path | get --optional package | default {}
      | get --optional version | default {})
    if not (($declared | describe) | str starts-with 'record') {
      error make { msg: $"($relative): package.version must inherit workspace.version" }
    }
    if ($declared | get --optional workspace | default false) != true {
      error make { msg: $"($relative): package.version must inherit workspace.version" }
    }
    let versions = ($locked | where name == $package.name | each {|entry| $entry.version })
    if $versions != [$version] {
      let found = if ($versions | is-empty) { 'missing' } else { $versions | str join ', ' }
      error make { msg: $"Cargo.lock: ($package.name) should be locked at ($version), found ($found)" }
    }
  }
  let members = ($packages | get name)
  for entry in ($dependencies | transpose alias spec) {
    if not (($entry.spec | describe) | str starts-with 'record') { continue }
    let name = ($entry.spec | get --optional package | default $entry.alias)
    if not ($name in $members) { continue }
    if ($entry.spec | get --optional version | default '') != $version {
      error make { msg: $"workspace dependency ($entry.alias) must carry version ($version)" }
    }
    if ($entry.spec | get --optional path | default '') == '' {
      error make { msg: $"workspace dependency ($entry.alias) must select the member path" }
    }
  }
  { version: $version, packages: ($packages | select name version manifest_path) }
}

# Third-party resolution, which a version bump must never change.
export def lock-external [root: string]: nothing -> list<record> {
  lock-packages $root
  | where {|entry| $entry.source? != null }
  | select name version source
  | sort-by name version source
}
