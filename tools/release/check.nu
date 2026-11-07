#!/usr/bin/env nu
# Verify a release source, its workspace packages, or the whole local release path.

use ../lib/repo.nu *
use ../lib/targets.nu *

const INDEX = 'https://index.crates.io'

# crates.io names crates by a path derived from the name, not by the name itself.
def index-path [name: string]: nothing -> string {
  let lower = ($name | str lowercase)
  let length = ($lower | str length)
  if $length == 1 { return $"1/($lower)" }
  if $length == 2 { return $"2/($lower)" }
  if $length == 3 { return $"3/($lower | str substring 0..1)/($lower)" }
  $"($lower | str substring 0..2)/($lower | str substring 2..4)/($lower)"
}

def published-versions [name: string]: nothing -> list<string> {
  let response = (http get --allow-errors --full --raw $"($INDEX)/(index-path $name)")
  if $response.status == 404 { return [] }
  if $response.status != 200 {
    error make { msg: $"cannot check crates.io package ($name): HTTP ($response.status)" }
  }
  $response.body | lines | where { |line| ($line | str trim) != '' }
  | each { |line| $line | from json | get vers }
}

# A binary-only release is fine; a half-published workspace has no single interpretation.
def check-registry [root: string, version: string] {
  let names = (workspace-packages $root | get name)
  let hit = ($names | where { |name| $version in (published-versions $name) })
  if ($hit | is-empty) {
    print $"crates.io: no workspace package is published at ($version)."
  } else if ($hit | length) == ($names | length) {
    print $"crates.io: every workspace package is already published at ($version); the release only adds binaries."
  } else {
    error make { msg: $"crates.io: ($hit | length) of ($names | length) packages are published at ($version); refusing a half-published workspace" }
  }
}

def check-source [root: string, tag: string, commit: string] {
  require-clean $root
  let selected = (check-tag $root $tag ($commit | if $in == '' { null } else { $in }))
  let checked = (check-version $root)
  require-clean $root
  print $"Release source ($tag): checked commit ($selected), ($checked.packages | length) workspace packages."
}

def check-packages [root: string, version: string] {
  check-registry $root $version
  let target = (mktemp -d)
  try {
    let result = (with-env { CARGO_TARGET_DIR: $target, CARGO_BUILD_BUILD_DIR: $target } {
      ^cargo package --workspace --locked --all-features --registry crates-io | complete
    })
    if $result.exit_code != 0 {
      error make { msg: $"cargo package failed: ($result.stderr | str trim)" }
    }
  } finally {
    rm -rf $target
  }
  print $"Workspace packages for ($version) passed Cargo packaging; nothing was uploaded."
}

def main [command: string, tag?: string, --commit: string = ''] {
  let root = ($env.FILE_PWD | path dirname | path dirname)
  match $command {
    'source' => {
      if $tag == null { error make { msg: 'source needs a tag' } }
      check-source $root $tag $commit
    }
    'packages' => {
      require-clean $root
      let checked = (check-version $root)
      check-packages $root $checked.version
      require-clean $root
    }
    'all' => {
      if $tag == null { error make { msg: 'all needs a tag' } }
      check-source $root $tag $commit
      let checked = (check-version $root)
      let ci = (^just ci | complete)
      if $ci.exit_code != 0 { error make { msg: 'just ci failed' } }
      check-packages $root $checked.version
      let package = ($root | path join 'tools/release/package.nu')
      let build = (^nu $package build --output ($root | path join 'release') | complete)
      if $build.exit_code != 0 { error make { msg: 'local release build failed' } }
      require-clean $root
      print $"Local release checks passed for ($tag). Cross-platform CI must also pass before publication."
    }
    _ => { error make { msg: $"unknown command: ($command)" } }
  }
}
