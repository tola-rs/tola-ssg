# Cargo release versions and tags.

export def parse-version [value: string]: nothing -> any {
  try { $value | into semver } catch {
    error make { msg: $"invalid SemVer version: '($value)'" }
  }
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

# Artifact names and version precedence both omit build metadata.
export def without-build-metadata [value: string]: nothing -> string {
  parse-version $value | ignore
  $value | split row '+' | first
}

export def version-precedence [value: string]: nothing -> any {
  without-build-metadata $value | into semver
}

export def is-prerelease [value: string]: nothing -> bool {
  parse-version $value | get pre | is-not-empty
}
