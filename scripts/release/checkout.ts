import { spawnSync } from 'node:child_process'
import { chmodSync, copyFileSync, lstatSync, mkdirSync, readlinkSync, symlinkSync, utimesSync } from 'node:fs'
import { isAbsolute, relative, resolve, sep } from 'node:path'
import { setImmediate } from 'node:timers/promises'
import { run } from './command.ts'
import { ReleaseError } from './release-error.ts'
import type { ReleaseMode } from './release-mode.ts'
import { tagVersion } from './semver.ts'
import { workspaceVersion } from './workspace.ts'

export function gitStatus(root: string): string {
  return run(root, ['git', 'status', '--porcelain=v1', '--untracked-files=all'])
}

export function requireClean(root: string): void {
  if (gitStatus(root)) {
    throw new ReleaseError(
      'working tree is dirty; commit or otherwise preserve your changes before updating a version or checking a release (preview and version-check are read-only)',
    )
  }
}

export function checkTag(root: string, tag: string, commit?: string, mode: ReleaseMode = 'create'): string {
  const expected = tagVersion(tag)
  run(root, ['git', 'check-ref-format', `refs/tags/${tag}`])
  const actual = workspaceVersion(root)
  if (expected !== actual) throw new ReleaseError(`tag ${tag} does not match workspace version ${actual}`)
  const head = run(root, ['git', 'rev-parse', '--verify', 'HEAD^{commit}'])
  if (commit !== undefined) {
    if ((commit.length !== 40 && commit.length !== 64) || /[^0-9a-fA-F]/.test(commit)) {
      throw new ReleaseError('release commit must be a full immutable Git object ID')
    }
    const selected = run(root, ['git', 'rev-parse', '--verify', `${commit}^{commit}`])
    if (selected !== head) throw new ReleaseError(`checkout ${head} differs from release commit ${selected}`)
  }
  const branches = run(root, ['git', 'for-each-ref', '--format=%(refname)', `refs/heads/${tag}`])
  if (branches.split('\n').includes(`refs/heads/${tag}`)) {
    throw new ReleaseError(`release tag conflicts with a branch: ${tag}`)
  }
  const tags = run(root, ['git', 'for-each-ref', '--format=%(refname)', `refs/tags/${tag}`])
  if (tags.split('\n').includes(`refs/tags/${tag}`)) {
    const tagged = run(root, ['git', 'rev-parse', '--verify', `refs/tags/${tag}^{commit}`])
    if (mode === 'create' && tagged !== head) {
      throw new ReleaseError(`tag ${tag} points to ${tagged}, not checked commit ${head}`)
    }
  }
  return head
}

/** Copy tracked and nonignored source as it exists, without following file symlinks. */
export async function copyCheckout(root: string, destination: string, signal?: AbortSignal): Promise<void> {
  await setImmediate()
  signal?.throwIfAborted()
  const completed = spawnSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'], {
    cwd: root,
    maxBuffer: 64 * 1024 * 1024,
  })
  await setImmediate()
  signal?.throwIfAborted()
  if (completed.error || completed.status !== 0) {
    throw new ReleaseError(
      `cannot enumerate checkout source: ${
        completed.error?.message ?? completed.stderr.toString('utf8').trim()
      }`,
    )
  }
  const paths = new Map<string, Buffer>()
  let start = 0
  for (let end = completed.stdout.indexOf(0); end !== -1; end = completed.stdout.indexOf(0, start)) {
    const encoded = completed.stdout.subarray(start, end)
    if (encoded.length) paths.set(encoded.toString('hex'), encoded)
    start = end + 1
  }
  if (start !== completed.stdout.length) throw new ReleaseError('Git returned an unterminated source path')
  const sourceRoot = resolve(root)
  const sourcePrefix = Buffer.from(`${sourceRoot}${sep}`)
  const targetPrefix = Buffer.from(`${resolve(destination)}${sep}`)
  for (const encoded of paths.values()) {
    await setImmediate()
    signal?.throwIfAborted()
    const path = encoded.toString('utf8')
    const within = relative(sourceRoot, resolve(sourceRoot, path))
    if (within === '..' || within.startsWith(`..${sep}`) || isAbsolute(within)) {
      throw new ReleaseError(`source path escapes checkout: ${path}`)
    }
    const source = Buffer.concat([sourcePrefix, encoded])
    const status = lstatSync(source, { throwIfNoEntry: false })
    if (!status) continue
    if (status.isDirectory()) {
      throw new ReleaseError(`cannot snapshot a Git submodule for version updates: ${path}`)
    }
    const target = Buffer.concat([targetPrefix, encoded])
    const separator = encoded.lastIndexOf(0x2f)
    // `mkdirSync` is the one `node:fs` entry point that takes no Buffer path, so the directory is
    // created from the same bytes as a string; every other call keeps the byte-exact path.
    mkdirSync(
      (separator === -1 ? targetPrefix : Buffer.concat([targetPrefix, encoded.subarray(0, separator)]))
        .toString('utf8'),
      { recursive: true },
    )
    if (status.isSymbolicLink()) {
      symlinkSync(readlinkSync(source, { encoding: 'buffer' }), target)
    } else {
      if (!status.isFile()) throw new ReleaseError(`cannot copy a non-regular source file: ${path}`)
      copyFileSync(source, target)
      chmodSync(target, status.mode & 0o777)
      utimesSync(target, status.atime, status.mtime)
    }
  }
  await setImmediate()
  signal?.throwIfAborted()
}
