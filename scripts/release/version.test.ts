import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { workspacePackages } from './cargo-metadata.ts'
import { checkTag, copyCheckout, gitStatus } from './checkout.ts'
import { updatedManifest } from './manifest.ts'
import { ReleaseError } from './release-error.ts'
import { checkSource } from './release-check.ts'
import { parseReleaseMode } from './release-mode.ts'
import { git, withGitCheckout, withWorkspace, WORKSPACE_MANIFEST } from './test-checkout.ts'
import { prepareUpdate, updateVersion } from './version-update.ts'
import { checkManifests } from './workspace.ts'

test('preview is read-only and Cargo updates every member before application', {
  ignore: process.platform === 'win32',
}, async () => {
  await withWorkspace(async (root) => {
    const originals = new Map(
      ['Cargo.toml', 'Cargo.lock'].map((name) => [name, readFileSync(join(root, name))]),
    )
    await updateVersion(root, '0.9.0-rc.1')
    for (const [name, original] of originals) expect(readFileSync(join(root, name))).toEqual(original)
    expect(gitStatus(root)).toBe('')
    await updateVersion(root, '0.9.0-rc.1', true)
    expect((await workspacePackages(root)).map((item) => item.version)).toEqual([
      '0.9.0-rc.1',
      '0.9.0-rc.1',
    ])
    const changed = new Map(Array.from(originals.keys(), (name) => [name, readFileSync(join(root, name))]))
    expect((await prepareUpdate(root, '0.9.0-rc.1')).replacements.size).toBe(0)
    for (const [name, content] of changed) expect(readFileSync(join(root, name))).toEqual(content)
  })
})

test('a dirty checkout cannot be modified by a version update', async () => {
  await withWorkspace(async (root) => {
    writeFileSync(join(root, 'src', 'main.rs'), 'fn main() { /* unsaved work */ }\n')
    const originals = new Map(
      ['Cargo.toml', 'Cargo.lock', 'src/main.rs'].map((name) => [name, readFileSync(join(root, name))]),
    )
    await expect(updateVersion(root, '0.9.0', true)).rejects.toThrow(ReleaseError)
    for (const [name, original] of originals) expect(readFileSync(join(root, name))).toEqual(original)
  })
})

test('target-specific aliases must inherit the synchronized member dependency', async () => {
  await withWorkspace(async (root) => {
    const packages = await workspacePackages(root)
    writeFileSync(
      join(root, 'Cargo.toml'),
      WORKSPACE_MANIFEST.replace(
        "[target.'cfg(unix)'.build-dependencies]\ntypes.workspace = true",
        '[target.\'cfg(unix)\'.build-dependencies]\ntypes = { package = "release-types", version = "0.8.0", path = "types" }',
      ),
    )
    expect(() => checkManifests(root, packages)).toThrow(ReleaseError)
  })
})

test('workspace aliases cannot redirect a member name or hide it behind another package name', async () => {
  await withWorkspace(async (root) => {
    const packages = await workspacePackages(root)
    for (
      const changed of [
        WORKSPACE_MANIFEST.replace('package = "release-types"', 'package = "another-name"'),
        WORKSPACE_MANIFEST.replace('path = "types"', 'path = "."'),
        WORKSPACE_MANIFEST.replace('version = "0.8.0", path', 'version = "0.7.0", path'),
      ]
    ) {
      writeFileSync(join(root, 'Cargo.toml'), changed)
      expect(() => checkManifests(root, packages)).toThrow(ReleaseError)
    }
  })
})

test('preview refuses a symlinked lockfile before staging source', async () => {
  await withWorkspace(async (root) => {
    const original = readFileSync(join(root, 'Cargo.lock'))
    renameSync(join(root, 'Cargo.lock'), join(root, 'saved-lock'))
    symlinkSync(join(root, 'saved-lock'), join(root, 'Cargo.lock'), 'file')
    await expect(prepareUpdate(root, '0.9.0')).rejects.toThrow(ReleaseError)
    expect(readFileSync(join(root, 'saved-lock'))).toEqual(original)
  })
})

test('downgrades and Git-unrepresentable SemVer values do not change source', async () => {
  await withWorkspace(async (root) => {
    const original = readFileSync(join(root, 'Cargo.toml'))
    for (const version of ['0.7.0', '0.9.0-rc.lock']) {
      await expect(prepareUpdate(root, version)).rejects.toThrow(ReleaseError)
    }
    expect(readFileSync(join(root, 'Cargo.toml'))).toEqual(original)
    writeFileSync(
      join(root, 'Cargo.toml'),
      updatedManifest(WORKSPACE_MANIFEST, '0.9.0-rc.lock', new Set(['release-cli', 'release-types'])),
    )
    expect(() => checkTag(root, 'v0.9.0-rc.lock')).toThrow(ReleaseError)
  })
})

test('missing tags are allowed but existing tags and immutable commit IDs must match HEAD', async () => {
  await withGitCheckout((root) => {
    const selected = checkTag(root, 'v0.8.0')
    git(root, 'tag', '-a', 'v0.8.0', '-m', 'release')
    expect(checkTag(root, 'v0.8.0', selected)).toBe(selected)
    for (const commit of ['HEAD', selected.slice(0, 12), `${selected}\n`]) {
      expect(() => checkTag(root, 'v0.8.0', commit)).toThrow(ReleaseError)
    }
    writeFileSync(join(root, 'changed'), 'changed')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'changed')
    expect(() => checkTag(root, 'v0.8.0')).toThrow(ReleaseError)
    expect(() => checkTag(root, 'v0.8.0', selected)).toThrow(ReleaseError)
  })
})

test('a branch with the release name blocks tag selection', async () => {
  await withGitCheckout((root) => {
    git(root, 'branch', 'v0.8.0')
    expect(() => checkTag(root, 'v0.8.0')).toThrow(ReleaseError)
  })
})

test('release modes reject unknown publication choices', () => {
  expect(parseReleaseMode(undefined)).toBe('create')
  for (const mode of ['create', 'update-preserve-notes', 'update-regenerate-notes']) {
    expect(parseReleaseMode(mode)).toBe(mode)
  }
  for (const mode of ['', 'update', 'force', 'CREATE']) {
    expect(() => parseReleaseMode(mode)).toThrow(ReleaseError)
  }
})

test('release updates validate selected source without moving tags', async () => {
  await withWorkspace(async (root) => {
    const tagged = git(root, 'rev-parse', 'HEAD')
    git(root, 'tag', '-a', 'v0.8.0', '-m', 'release')
    git(root, 'commit', '--allow-empty', '-qm', 'fix: selected source')
    const selected = git(root, 'rev-parse', 'HEAD')
    for (const mode of ['update-preserve-notes', 'update-regenerate-notes'] as const) {
      expect((await checkSource(root, 'v0.8.0', selected, undefined, mode)).commit).toBe(selected)
      expect(git(root, 'rev-parse', 'v0.8.0^{commit}')).toBe(tagged)
      await expect(checkSource(root, 'v0.8.0', tagged, undefined, mode)).rejects.toThrow(ReleaseError)
      await expect(checkSource(root, 'v0.9.0', selected, undefined, mode)).rejects.toThrow(ReleaseError)
      for (const commit of ['HEAD', selected.slice(0, 12)]) {
        expect(() => checkTag(root, 'v0.8.0', commit, mode)).toThrow(ReleaseError)
      }
    }
    await expect(checkSource(root, 'v0.8.0', selected)).rejects.toThrow(ReleaseError)
    writeFileSync(join(root, 'changed'), 'unsaved source')
    await expect(checkSource(root, 'v0.8.0', selected, undefined, 'update-preserve-notes')).rejects.toThrow(
      ReleaseError,
    )
  })
})

test('release updates reject branch conflicts and noncommit tags', async () => {
  await withGitCheckout((root) => {
    const selected = git(root, 'rev-parse', 'HEAD')
    git(root, 'branch', 'v0.8.0')
    for (const mode of ['update-preserve-notes', 'update-regenerate-notes'] as const) {
      expect(() => checkTag(root, 'v0.8.0', selected, mode)).toThrow(ReleaseError)
    }
    git(root, 'branch', '-D', 'v0.8.0')
    git(root, 'tag', 'v0.8.0', git(root, 'rev-parse', 'HEAD^{tree}'))
    for (const mode of ['update-preserve-notes', 'update-regenerate-notes'] as const) {
      expect(() => checkTag(root, 'v0.8.0', selected, mode)).toThrow(ReleaseError)
    }
  })
})

test('source copies retain nonignored edits and symlinks, but exclude ignored files and tracked deletions', async () => {
  await withGitCheckout(async (root) => {
    writeFileSync(join(root, 'tracked'), 'original')
    writeFileSync(join(root, 'deleted'), 'delete me')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'source')
    writeFileSync(join(root, 'tracked'), 'edited source')
    rmSync(join(root, 'deleted'))
    writeFileSync(join(root, '.gitignore'), 'ignored\n')
    writeFileSync(join(root, 'ignored'), 'private ignored content')
    writeFileSync(join(root, 'untracked'), 'new source')
    symlinkSync('tracked', join(root, 'link'), 'file')
    const destination = mkdtempSync(join(tmpdir(), 'tola-source-copy-'))
    try {
      await copyCheckout(root, destination)
      expect(readFileSync(join(destination, 'tracked'), 'utf8')).toBe('edited source')
      expect(readFileSync(join(destination, 'untracked'), 'utf8')).toBe('new source')
      expect(existsSync(join(destination, 'ignored'))).toBe(false)
      expect(existsSync(join(destination, 'deleted'))).toBe(false)
      expect(lstatSync(join(destination, 'link')).isSymbolicLink()).toBe(true)
      expect(readlinkSync(join(destination, 'link'))).toBe('tracked')
    } finally {
      rmSync(destination, { recursive: true, force: true })
    }
  })
})

test('a tracked path that becomes a directory cannot be staged as a source snapshot', async () => {
  await withGitCheckout(async (root) => {
    writeFileSync(join(root, 'tracked'), 'source')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'source')
    rmSync(join(root, 'tracked'))
    mkdirSync(join(root, 'tracked'))
    const destination = mkdtempSync(join(tmpdir(), 'tola-source-copy-'))
    try {
      await expect(copyCheckout(root, destination)).rejects.toThrow(ReleaseError)
    } finally {
      rmSync(destination, { recursive: true, force: true })
    }
  })
})
