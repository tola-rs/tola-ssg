import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import { existsSync, readFileSync } from 'node:fs'
import { basename, join } from 'node:path'
import { applyFiles, atomicReplace, withUpdateLock } from './checked-files.ts'
import { checkAvailable } from './registry.ts'
import { ReleaseError } from './release-error.ts'
import { versionFiles, withGitCheckout } from './test-checkout.ts'
import { updateVersion } from './version-update.ts'

test('cancellation after a rename restores owned writes before releasing the update lock', async () => {
  await withGitCheckout(async (root) => {
    const { originals, replacements } = versionFiles(root)
    const controller = new AbortController()
    const reason = new Error('cancel release')
    await expect(
      withUpdateLock(root, () =>
        applyFiles(
          root,
          originals,
          replacements,
          (path, content, expected) => {
            atomicReplace(path, content, expected)
            if (basename(path) === 'Cargo.lock' && content.toString() === 'new lock') controller.abort(reason)
          },
          controller.signal,
        )),
    ).rejects.toBe(reason)
    for (const [path, content] of originals) expect(readFileSync(join(root, path)).equals(content)).toBe(true)
    expect(existsSync(join(root, '.git', 'tola-version.lock'))).toBe(false)
  })
})

test('exclusive ownership lasts until asynchronous work and its failure have settled', async () => {
  await withGitCheckout(async (root) => {
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const reason = new Error('asynchronous preparation failed')
    const owner = withUpdateLock(root, async () => {
      entered.resolve()
      await release.promise
      throw reason
    })
    const rejected = owner.catch((error: unknown) => error)
    await entered.promise
    try {
      await expect(withUpdateLock(root, () => undefined)).rejects.toThrow(ReleaseError)
    } finally {
      release.resolve()
      expect(await rejected).toBe(reason)
    }
    expect(existsSync(join(root, '.git', 'tola-version.lock'))).toBe(false)
  })
})

test('a pre-cancelled update never takes ownership or changes source', async () => {
  await withGitCheckout(async (root) => {
    const original = readFileSync(join(root, 'Cargo.toml'))
    const reason = new Error('cancel before preparation')
    await expect(updateVersion(root, '0.9.0', true, AbortSignal.abort(reason))).rejects.toBe(reason)
    expect(readFileSync(join(root, 'Cargo.toml'))).toEqual(original)
    expect(existsSync(join(root, '.git', 'tola-version.lock'))).toBe(false)
  })
})

test('cancelling an in-flight registry request is not reported as registry unavailability', async () => {
  const entered = Promise.withResolvers<void>()
  const response = Promise.withResolvers<Response>()
  const server = Deno.serve({
    hostname: '127.0.0.1',
    port: 0,
    onListen: () => {},
    handler() {
      entered.resolve()
      return response.promise
    },
  })
  const controller = new AbortController()
  const reason = new Error('cancel registry request')
  try {
    const result = checkAvailable(
      [{ id: 'release', name: 'release-crate', version: '0.8.0', manifestPath: 'Cargo.toml', publish: null }],
      '0.8.0',
      (_url, options) => fetch(`http://127.0.0.1:${server.addr.port}/`, options),
      controller.signal,
    )
    const rejected = result.catch((error: unknown) => error)
    await entered.promise
    controller.abort(reason)
    expect(await rejected).toBe(reason)
  } finally {
    controller.abort(reason)
    response.resolve(new Response('cancelled', { status: 503 }))
    await server.shutdown()
  }
})
