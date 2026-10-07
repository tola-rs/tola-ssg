import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  commitSubjects,
  findPreviousTag,
  groupedSubjects,
  normalizedSubject,
  renderNotes,
} from './release-notes.ts'
import { ReleaseError } from './release-error.ts'
import { git, withGitCheckout } from './test-checkout.ts'

test('conventional scopes and breaking markers are removed only for user-facing kinds', () => {
  expect(normalizedSubject('  FIX(cli)!:   preserve   output  ')).toEqual({
    category: 'Bug Fixes',
    body: 'preserve output',
  })
  expect(normalizedSubject('perf(css): faster matching')).toEqual({
    category: 'Performance',
    body: 'faster matching',
  })
  expect(normalizedSubject('unknown(scope): retain the full subject')).toEqual({
    category: 'Other Changes',
    body: 'unknown(scope): retain the full subject',
  })
  for (
    const subject of [
      '',
      'ci(build): checks',
      'docs: wording',
      'release 0.8.0',
      'v0.8.0-rc.1',
      'Update README examples',
      'docs examples',
    ]
  ) {
    expect(normalizedSubject(subject)).toBeNull()
  }
})

test('deduplication uses full Unicode case folding within each category', () => {
  const groups = groupedSubjects([
    'feat: Straße',
    'feat: STRASSE',
    'feat: ος',
    'feat: οσ',
    'feat: ﬃ',
    'feat: FFI',
    'feat: Ꭰ',
    'feat: ꭰ',
    'fix: STRASSE',
  ])
  expect(groups.Features).toEqual(['Straße', 'ος', 'ﬃ', 'Ꭰ'])
  expect(groups['Bug Fixes']).toEqual(['STRASSE'])
})

test('note whitespace includes NEL and information separators but preserves a literal BOM', () => {
  expect(normalizedSubject('\u0085feat:\u001cwide\u2003space\u001f')).toEqual({
    category: 'Features',
    body: 'wide space',
  })
  expect(normalizedSubject('feat: \ufeffkeep\ufeff')).toEqual({
    category: 'Features',
    body: '\ufeffkeep\ufeff',
  })
})

test('previous tags use SemVer ordering across all release lines rather than ancestry or lexical order', async () => {
  await withGitCheckout(async (root) => {
    for (const tag of ['v0.7.0', 'v0.8.0-rc.2', 'v0.8.0-rc.10', 'v0.8.0', 'v0.08.0', 'v1.0.0']) {
      git(root, 'tag', tag)
    }
    expect(await findPreviousTag(root, 'v0.8.0')).toBe('v0.8.0-rc.10')
    expect(await findPreviousTag(root, 'v0.8.0-rc.3')).toBe('v0.8.0-rc.2')
    expect(await findPreviousTag(root, 'v0.7.0')).toBeNull()
  })
})

test('rebased patch-equivalent changes are excluded from release notes', async () => {
  await withGitCheckout(async (root) => {
    const base = git(root, 'rev-parse', 'HEAD')
    writeFileSync(join(root, 'feature'), 'shared change\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'feat: shared feature')
    git(root, 'tag', 'v0.7.0')
    git(root, 'checkout', '-qb', 'release-line', base)
    writeFileSync(join(root, 'feature'), 'shared change\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'feat: rebased shared feature')
    writeFileSync(join(root, 'fix'), 'new fix\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'fix(cli)!: preserve output')
    git(root, 'tag', 'v0.8.0')
    expect(await commitSubjects(root, 'v0.7.0', 'v0.8.0')).toEqual(['fix(cli)!: preserve output'])
    expect(await renderNotes(root, 'v0.8.0', 'tola-rs/tola-ssg')).toBe(
      "## What's Changed\n\n### Bug Fixes\n- preserve output\n\n**Full Changelog**: https://github.com/tola-rs/tola-ssg/compare/v0.7.0...v0.8.0\n",
    )
  })
})

test('initial notes use the selected commit message without creating a missing tag', async () => {
  await withGitCheckout(async (root) => {
    git(root, 'commit', '--allow-empty', '-qm', 'First release\n\nFull Unicode message: Straße.')
    const selected = git(root, 'rev-parse', 'HEAD')
    expect(await renderNotes(root, 'v0.8.0', 'tola-rs/tola-ssg', selected)).toBe(
      'First release\n\nFull Unicode message: Straße.\n',
    )
    expect(git(root, 'tag', '--list')).toBe('')
    git(root, 'commit', '--allow-empty', '-qm', 'Later work')
    await expect(renderNotes(root, 'v0.8.0', 'tola-rs/tola-ssg', selected)).rejects.toThrow(ReleaseError)
  })
})

test('an existing historical tag selects its own message rather than the current checkout', async () => {
  await withGitCheckout(async (root) => {
    git(root, 'commit', '--allow-empty', '-qm', 'Published release message')
    git(root, 'tag', 'v0.8.0')
    git(root, 'commit', '--allow-empty', '-qm', 'Unreleased work')
    expect(await renderNotes(root, 'v0.8.0', 'tola-rs/tola-ssg')).toBe('Published release message\n')
  })
})

test('updated release notes use the selected commit', async () => {
  await withGitCheckout(async (root) => {
    git(root, 'tag', 'v0.7.0')
    git(root, 'commit', '--allow-empty', '-qm', 'feat: published feature')
    git(root, 'tag', '-a', 'v0.8.0', '-m', 'release')
    const tagged = git(root, 'rev-parse', 'v0.8.0^{commit}')
    git(root, 'commit', '--allow-empty', '-qm', 'fix: selected correction')
    const selected = git(root, 'rev-parse', 'HEAD')
    for (const mode of ['update-preserve-notes', 'update-regenerate-notes'] as const) {
      const notes = await renderNotes(root, 'v0.8.0', 'tola-rs/tola-ssg', selected, undefined, mode)
      expect(notes).toContain('published feature')
      expect(notes).toContain('selected correction')
      expect(git(root, 'rev-parse', 'v0.8.0^{commit}')).toBe(tagged)
    }
    await expect(renderNotes(root, 'v0.8.0', 'tola-rs/tola-ssg', selected)).rejects.toThrow(ReleaseError)
  })
})
