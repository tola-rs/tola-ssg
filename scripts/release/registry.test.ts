import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import type { WorkspacePackage } from './cargo-metadata.ts'
import { checkAvailable, registryPath } from './registry.ts'
import { ReleaseError } from './release-error.ts'

const PACKAGE: WorkspacePackage = {
  id: 'release-crate',
  name: 'release-crate',
  version: '0.8.0',
  manifestPath: 'Cargo.toml',
  publish: null,
}

test('the sparse index path covers all crate-name length boundaries', () => {
  expect(['A', 'AB', 'ABC', 'ABCD', 'Release_Crate'].map(registryPath)).toEqual([
    '1/a',
    '2/ab',
    '3/a/abc',
    'ab/cd/abcd',
    're/le/release_crate',
  ])
  expect(() => registryPath('../crate')).toThrow(ReleaseError)
})

test('published versions collide despite build metadata, aliases, casing, or yanking', async () => {
  await expect(
    checkAvailable(
      [PACKAGE],
      '0.8.0+replacement',
      () =>
        Promise.resolve(
          new Response(
            `${JSON.stringify({ name: 'RELEASE_CRATE', vers: '0.8.0+published', yanked: true })}\n`,
          ),
        ),
    ),
  ).rejects.toThrow(`${PACKAGE.name} 0.8.0+replacement`)
})

test('registry HTTP failures, transport failures, and invalid UTF-8 stop checks', async () => {
  await expect(
    checkAvailable(
      [PACKAGE],
      '0.8.0',
      () =>
        Promise.resolve(new Response(JSON.stringify({ name: PACKAGE.name, vers: '0.7.0' }), { status: 503 })),
    ),
  ).rejects.toThrow(ReleaseError)
  await expect(
    checkAvailable([PACKAGE], '0.8.0', () => Promise.reject(new Error('connection reset'))),
  ).rejects.toThrow(ReleaseError)
  const invalidUtf8 = Buffer.concat([
    Buffer.from(`{"name":"${PACKAGE.name}","vers":"0.7.0","extra":"`),
    Buffer.from([0xff]),
    Buffer.from('"}'),
  ])
  await expect(checkAvailable([PACKAGE], '0.8.0', () => Promise.resolve(new Response(invalidUtf8)))).rejects
    .toThrow(
      ReleaseError,
    )
})

test('malformed, empty, or mismatched sparse index entries are not treated as availability', async () => {
  for (
    const body of [
      '',
      '  \n',
      'not JSON',
      'null',
      '[]',
      JSON.stringify({ name: 'another-crate', vers: '0.7.0' }),
      JSON.stringify({ name: PACKAGE.name }),
      JSON.stringify({ name: PACKAGE.name, vers: '0.7.0-01' }),
    ]
  ) {
    await expect(checkAvailable([PACKAGE], '0.8.0', () => Promise.resolve(new Response(body)))).rejects
      .toThrow(
        ReleaseError,
      )
  }
})

test('an absent package does not hide a later occupied version', async () => {
  const missing: WorkspacePackage = { ...PACKAGE, id: 'absent', name: 'absent-crate' }
  await expect(
    checkAvailable(
      [missing, PACKAGE],
      '0.8.0',
      (url) =>
        Promise.resolve(
          url.endsWith('/absent-crate')
            ? new Response('missing', { status: 404 })
            : new Response(JSON.stringify({ name: PACKAGE.name, vers: '0.8.0' })),
        ),
    ),
  ).rejects.toThrow(`${PACKAGE.name} 0.8.0`)
})
