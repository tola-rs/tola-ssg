# Semantic Versioning 2.0.0, restricted to the versions Cargo accepts.

const VERSION = '^(?<major>0|[1-9][0-9]*)\.(?<minor>0|[1-9][0-9]*)\.(?<patch>0|[1-9][0-9]*)(?:-(?<pre>[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+(?<build>[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$'
const MAX_CORE = '18446744073709551615'

# Compare decimal strings without narrowing Cargo's u64 core or an unbounded prerelease number.
def compare-decimal [left: string, right: string]: nothing -> int {
  let left_length = ($left | str length)
  let right_length = ($right | str length)
  if $left_length < $right_length { return (-1) }
  if $left_length > $right_length { return 1 }
  if $left == $right { return 0 }
  if $left < $right { -1 } else { 1 }
}

# Split a version into its numeric core and its prerelease identifiers.
export def parse-version [value: string]: nothing -> record {
  let parts = ($value | parse --regex $VERSION | first)
  if ($parts | is-empty) { error make { msg: $"invalid SemVer version: '($value)'" } }
  let core = [$parts.major $parts.minor $parts.patch]
  let pre = $parts.pre? | default '' | if $in == '' { [] } else { split row '.' }
  if ($core | any {|part| (compare-decimal $part $MAX_CORE) > 0 }) or ($pre | any {|part| $part =~ '^0[0-9]+$' }) {
    error make { msg: $"invalid SemVer version: '($value)'" }
  }
  { core: $core, prerelease: $pre }
}

# Compare two parsed versions: -1 when the left is older, 1 when it is newer.
export def compare-versions [left: record, right: record]: nothing -> int {
  for index in 0..2 {
    let order = (compare-decimal ($left.core | get $index) ($right.core | get $index))
    if $order != 0 { return $order }
  }
  let left_pre = $left.prerelease
  let right_pre = $right.prerelease
  if ($left_pre | is-empty) and ($right_pre | is-empty) { return 0 }
  if ($left_pre | is-empty) { return 1 }
  if ($right_pre | is-empty) { return (-1) }
  let limit = ([($left_pre | length) ($right_pre | length)] | math min)
  for index in 0..<$limit {
    let first = ($left_pre | get $index)
    let second = ($right_pre | get $index)
    if $first == $second { continue }
    let first_numeric = $first =~ '^[0-9]+$'
    let second_numeric = $second =~ '^[0-9]+$'
    if $first_numeric and $second_numeric { return (compare-decimal $first $second) }
    if $first_numeric { return (-1) }
    if $second_numeric { return 1 }
    if $first < $second { return (-1) }
    return 1
  }
  (($left_pre | length) - ($right_pre | length)) | if $in < 0 { -1 } else if $in > 0 { 1 } else { 0 }
}

# The version a release tag names; the tag is the version with a leading v.
export def tag-version [tag: string]: nothing -> string {
  if not ($tag | str starts-with 'v') {
    error make { msg: $"release tag must start with v: '($tag)'" }
  }
  let value = ($tag | str substring 1..)
  parse-version $value | ignore
  $value
}

# Build metadata never names an artifact, so one version names one archive.
export def without-build-metadata [value: string]: nothing -> string {
  parse-version $value | ignore
  let separator = ($value | str index-of '+')
  if $separator < 0 { $value } else { $value | str substring ..<$separator }
}

# A prerelease version is the only kind a release marks as such.
export def is-prerelease [value: string]: nothing -> bool {
  parse-version $value | get prerelease | is-not-empty
}
