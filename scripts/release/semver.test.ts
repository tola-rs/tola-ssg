import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import { ReleaseError } from './release-error.ts'
import { compareVersions, tagVersion, versionKey } from './semver.ts'

test('SemVer rejects ambiguous and non-ASCII numeric syntax', () => {
  for (
    const value of [
      '0.08.0',
      '0.8',
      '0.8.0-01',
      '0.8.0-',
      '0.8.0\n',
      'v0.8.0',
      '0.8.0-β',
      '0.8.0+',
      '0.8.0-rc..1',
    ]
  ) {
    expect(() => versionKey(value)).toThrow(ReleaseError)
  }
  expect(() => tagVersion('0.8.0')).toThrow(ReleaseError)
})

test('Cargo core components retain exact u64 boundaries', () => {
  const maximum = '18446744073709551615'
  expect(versionKey(`${maximum}.${maximum}.${maximum}`).core).toEqual([
    18446744073709551615n,
    18446744073709551615n,
    18446744073709551615n,
  ])
  for (const value of ['18446744073709551616.0.0', '0.18446744073709551616.0', '0.0.18446744073709551616']) {
    expect(() => versionKey(value)).toThrow(ReleaseError)
  }
  expect(compareVersions(versionKey('9007199254740992.0.0'), versionKey('9007199254740993.0.0'))).toBe(-1)
})

test('prerelease numbers are unbounded and metadata has no precedence', () => {
  expect(
    compareVersions(
      versionKey('0.8.0-rc.184467440737095516160'),
      versionKey('0.8.0-rc.184467440737095516161'),
    ),
  ).toBe(-1)
  expect(compareVersions(versionKey('0.8.0+build.01'), versionKey('0.8.0+build.02'))).toBe(0)
  const ordered = [
    '1.0.0-alpha',
    '1.0.0-alpha.1',
    '1.0.0-alpha.beta',
    '1.0.0-beta',
    '1.0.0-beta.2',
    '1.0.0-beta.11',
    '1.0.0-rc.1',
    '1.0.0',
  ]
  for (let index = 1; index < ordered.length; index++) {
    const previous = ordered[index - 1]
    const current = ordered[index]
    if (previous === undefined || current === undefined) throw new Error('incomplete SemVer ordering case')
    expect(compareVersions(versionKey(previous), versionKey(current))).toBe(-1)
    expect(compareVersions(versionKey(current), versionKey(previous))).toBe(1)
  }
})
