#!/usr/bin/env nu
# Verify a release source, its workspace packages, or the whole local release path.

use ../lib/repo.nu *
use ../lib/semver.nu [without-build-metadata]

const INDEX = 'https://index.crates.io'

# crates.io names crates by a path derived from the name, not by the name itself.
def index-path [name: string]: nothing -> string {
  let lower = ($name | str lowercase)
  let length = ($lower | str length)
  if $length == 1 { return $"1/($lower)" }
  if $length == 2 { return $"2/($lower)" }
  if $length == 3 { return $"3/($lower | str substring 0..<1)/($lower)" }
  $"($lower | str substring 0..<2)/($lower | str substring 2..<4)/($lower)"
}

def published-versions [name: string]: nothing -> list<string> {
  let response = (http get --allow-errors --full --raw $"($INDEX)/(index-path $name)")
  if $response.status == 404 { return [] }
  if $response.status != 200 {
    error make { msg: $"cannot check crates.io package ($name): HTTP ($response.status)" }
  }
  $response.body | lines | where { |line| ($line | str trim) != '' }
  | each { |line| without-build-metadata ($line | from json | get vers) }
}

# A binary-only release is fine; a half-published workspace has no single interpretation.
def check-registry [checked: record] {
  let version = $checked.version
  let plain = (without-build-metadata $version)
  let names = ($checked.packages | get name)
  let hit = ($names | where { |name| $plain in (published-versions $name) })
  if ($hit | is-empty) {
    print $"crates.io: no workspace package is published at ($version)."
  } else if ($hit | length) == ($names | length) {
    print $"crates.io: every workspace package is already published at ($version); the release only adds binaries."
  } else {
    error make { msg: $"crates.io: ($hit | length) of ($names | length) packages are published at ($version); refusing a half-published workspace" }
  }
}

def check-source [root: string, tag: string, commit: string, mode: string] {
  require-clean $root
  let selected = (check-tag $root $tag ($commit | if $in == '' { null } else { $in }) --mode $mode)
  let checked = (check-version $root)
  require-clean $root
  print $"Release source ($tag): checked commit ($selected), ($checked.packages | length) workspace packages."
  $checked
}

def check-packages [root: string, checked: record, mode: string] {
  if $mode == 'create' { check-registry $checked }
  cd $root
  ^cargo package --workspace --locked --all-features --registry crates-io
  print $"Workspace packages for ($checked.version) passed Cargo packaging; nothing was uploaded."
}

def main [command: string, tag?: string, --commit: string = '', --mode: string = 'create'] {
  release-mode $mode | ignore
  let root = ($env.FILE_PWD | path dirname | path dirname)
  match $command {
    'source' => {
      if $tag == null { error make { msg: 'source needs a tag' } }
      check-source $root $tag $commit $mode | ignore
    }
    'packages' => {
      if $tag == null { error make { msg: 'packages needs a tag' } }
      let checked = (check-source $root $tag $commit $mode)
      check-packages $root $checked $mode
      require-clean $root
    }
    'all' => {
      if $tag == null { error make { msg: 'all needs a tag' } }
      if $mode != 'create' { error make { msg: 'all only supports create mode' } }
      let checked = (check-source $root $tag $commit $mode)
      cd $root
      ^just ci
      check-packages $root $checked $mode
      let shell = $nu.current-exe
      ^$shell tools/release/package.nu build --output release
      require-clean $root
      print $"Local release checks passed for ($tag). Cross-platform CI must also pass before publication."
    }
    _ => { error make { msg: $"unknown command: ($command)" } }
  }
}
