import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import { parseToml, TomlTimestamp } from './cargo-toml.ts'
import { updatedManifest } from './manifest.ts'
import { ReleaseError } from './release-error.ts'
import { WORKSPACE_MANIFEST } from './test-checkout.ts'
import { checkExternalResolution } from './workspace.ts'

const MEMBERS = new Set(['release-cli', 'release-types'])

test('version edits preserve aliases, features, comments, and CRLF bytes', () => {
  const source = WORKSPACE_MANIFEST.replaceAll('\n', '\r\n')
  const changed = updatedManifest(source, '0.9.0-rc.1', MEMBERS)
  expect(changed).toBe(source.replaceAll('"0.8.0"', '"0.9.0-rc.1"'))
})

test('TOML syntax, not release-looking text inside values, selects version fields', () => {
  const source = `[workspace]
members = [".", "types"]
metadata.example = '''
[workspace.package]
version = "not a release"
'''
[workspace.package]
# version = "not a release"
version = '0.8.0' # keep the comment
[workspace.dependencies.'types']
package = "release-types"
path = "types"
version = "0.8.0"
features = ["braces} and version=0.8.0"]
`
  expect(updatedManifest(source, '0.9.0', MEMBERS)).toBe(
    source
      .replace("version = '0.8.0'", 'version = "0.9.0"')
      .replace('version = "0.8.0"', 'version = "0.9.0"'),
  )
})

test('unrelated TOML preserves large integers, timestamps, nonfinite floats and nested arrays', () => {
  const source = `${WORKSPACE_MANIFEST}
[workspace.metadata]
maximum = 9223372036854775807
minimum = -9223372036854775808
nan = nan
positive = inf
negative = -inf
negative-zero = -0.0
time = 12:34:56.123456789
local = 2026-09-12T12:34:56.123456789
offset = 2026-09-12T12:34:56.123456789+08:00
text = ["a", { nested = "quoted } version = 0.8.0" }]
[[workspace.metadata.people]]
name = "Ada"
[workspace.metadata.people.details]
enabled = true
[[workspace.metadata.people]]
name = "Grace"
`
  expect(updatedManifest(source, '0.9.0', MEMBERS)).toBe(source.replaceAll('"0.8.0"', '"0.9.0"'))
  const decoded = parseToml('integer = 9223372036854775807\ntime = 12:34:56.123456789\nnan = nan\n').values
  expect(decoded.integer).toBe(9223372036854775807n)
  expect(decoded.time).toEqual(new TomlTimestamp('local-time', '12:34:56.123456789'))
  expect(decoded.nan).toBeNaN()
})

test('missing literal versions are rejected rather than synthesized', () => {
  expect(() =>
    updatedManifest(WORKSPACE_MANIFEST.replace('version = "0.8.0", path', 'path'), '0.9.0', MEMBERS)
  ).toThrow(ReleaseError)
})

test('third-party lock resolution includes every package field, independent of package order', () => {
  const first =
    `[[package]]\nname = "one"\nversion = "1.0.0"\nsource = "registry+https://example.invalid"\nchecksum = "abc"\ndependencies = ["two"]\n`
  const second =
    `[[package]]\nname = "two"\nversion = "2.0.0"\nsource = "registry+https://example.invalid"\nchecksum = "def"\n`
  const workspace = `[[package]]\nname = "workspace"\nversion = "0.8.0"\n`
  const before = parseToml(first + second + workspace).values
  checkExternalResolution(before, parseToml(second + first + workspace.replace('0.8.0', '0.9.0')).values)
  expect(() =>
    checkExternalResolution(
      before,
      parseToml((first + second).replace('checksum = "abc"', 'checksum = "changed"')).values,
    )
  ).toThrow(ReleaseError)
  expect(() =>
    checkExternalResolution(
      before,
      parseToml((first + second).replace('dependencies = ["two"]', 'dependencies = []')).values,
    )
  ).toThrow(ReleaseError)
})
