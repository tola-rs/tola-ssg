import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import { deepStrictEqual } from 'node:assert'
import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import { applyFiles, atomicReplace, withUpdateLock } from './checked-files.ts'
import { ReleaseError } from './release-error.ts'
import { versionFiles, withGitCheckout } from './test-checkout.ts'

async function withVersionFiles(
  operation: (
    root: string,
    originals: Map<string, Buffer>,
    replacements: Map<string, Buffer>,
  ) => void | Promise<void>,
): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), 'tola-version-write-'))
  try {
    const { originals, replacements } = versionFiles(root)
    await operation(root, originals, replacements)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

test('an error after the second rename restores both owned writes', async () => {
  await withVersionFiles(async (root, originals, replacements) => {
    await expect(
      applyFiles(root, originals, replacements, (path, content, expected) => {
        atomicReplace(path, content, expected)
        if (basename(path) === 'Cargo.lock' && content.toString() === 'new lock') {
          throw new Error('interrupted after replacement')
        }
      }),
    ).rejects.toThrow('interrupted after replacement')
    for (const [path, original] of originals) deepStrictEqual(readFileSync(join(root, path)), original)
  })
})

test('a concurrent edit between replacements is preserved and the first write is restored', async () => {
  await withVersionFiles(async (root, originals, replacements) => {
    await expect(
      applyFiles(root, originals, replacements, (path, content, expected) => {
        atomicReplace(path, content, expected)
        if (basename(path) === 'Cargo.toml' && content.toString() === 'new manifest') {
          writeFileSync(join(root, 'Cargo.lock'), 'concurrent lock edit')
        }
      }),
    ).rejects.toThrow(ReleaseError)
    deepStrictEqual(readFileSync(join(root, 'Cargo.toml')), originals.get('Cargo.toml'))
    expect(readFileSync(join(root, 'Cargo.lock'), 'utf8')).toBe('concurrent lock edit')
  })
})

test('rollback cannot overwrite an edit made to an already replaced file', async () => {
  await withVersionFiles(async (root, originals, replacements) => {
    await expect(
      applyFiles(root, originals, replacements, (path, content, expected) => {
        atomicReplace(path, content, expected)
        if (basename(path) === 'Cargo.lock' && content.toString() === 'new lock') {
          writeFileSync(join(root, 'Cargo.toml'), 'concurrent manifest edit')
          throw new Error('interrupted after replacement')
        }
      }),
    ).rejects.toThrow('Cargo.toml: concurrent edit preserved')
    expect(readFileSync(join(root, 'Cargo.toml'), 'utf8')).toBe('concurrent manifest edit')
    deepStrictEqual(readFileSync(join(root, 'Cargo.lock')), originals.get('Cargo.lock'))
  })
})

test('an edit detected before application leaves every file untouched', async () => {
  await withVersionFiles(async (root, originals, replacements) => {
    writeFileSync(join(root, 'Cargo.toml'), 'concurrent edit')
    await expect(applyFiles(root, originals, replacements)).rejects.toThrow(ReleaseError)
    expect(readFileSync(join(root, 'Cargo.toml'), 'utf8')).toBe('concurrent edit')
    deepStrictEqual(readFileSync(join(root, 'Cargo.lock')), originals.get('Cargo.lock'))
  })
})

test('replacement refuses symlinks without changing their targets', async () => {
  await withVersionFiles((root) => {
    const target = join(root, 'Cargo.toml')
    const link = join(root, 'manifest-link')
    symlinkSync(target, link, 'file')
    expect(() => atomicReplace(link, Buffer.from('replacement'), Buffer.from('original manifest'))).toThrow(
      ReleaseError,
    )
    expect(readFileSync(target, 'utf8')).toBe('original manifest')
  })
})

test('the exclusive Git-path lock rejects another owner and is released on ordinary failures', async () => {
  await withGitCheckout(async (root) => {
    const lock = join(root, '.git', 'tola-version.lock')
    await expect(
      withUpdateLock(root, async () => {
        const owner = readFileSync(lock)
        await expect(
          withUpdateLock(root, () => {
            throw new Error('second owner entered')
          }),
        ).rejects.toThrow(ReleaseError)
        expect(readFileSync(lock)).toEqual(owner)
        throw new Error('preparation failed')
      }),
    ).rejects.toThrow('preparation failed')
    expect(existsSync(lock)).toBe(false)
  })
})
