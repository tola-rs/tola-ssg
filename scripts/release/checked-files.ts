import { randomUUID } from 'node:crypto'
import {
  closeSync,
  fchmodSync,
  fsyncSync,
  lstatSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { basename, dirname, isAbsolute, join } from 'node:path'
import { setImmediate } from 'node:timers/promises'
import { errorMessage } from '../process.ts'
import { run } from './command.ts'
import { ReleaseError } from './release-error.ts'

export type ReleaseFiles = ReadonlyMap<string, Buffer>
export type CheckedFileReplace = (path: string, content: Buffer, expected: Buffer) => void

export function readReleaseFile(path: string): Buffer {
  if (!lstatSync(path).isFile()) {
    throw new ReleaseError(`version preparation requires a regular file: ${path}`)
  }
  return readFileSync(path)
}

/** The final byte check protects detected edits; this is not filesystem compare-and-swap. */
export function atomicReplace(path: string, content: Buffer, expected: Buffer): void {
  const status = lstatSync(path)
  if (!status.isFile()) throw new ReleaseError(`refusing to replace a non-regular release file: ${path}`)
  const temporary = join(dirname(path), `.${basename(path)}.${randomUUID()}`)
  let descriptor: number | undefined
  try {
    descriptor = openSync(temporary, 'wx', 0o600)
    writeFileSync(descriptor, content)
    fsyncSync(descriptor)
    fchmodSync(descriptor, status.mode & 0o777)
    closeSync(descriptor)
    descriptor = undefined
    if (!lstatSync(path).isFile() || !readFileSync(path).equals(expected)) {
      throw new ReleaseError(`file changed before replacement: ${path}`)
    }
    renameSync(temporary, path)
  } finally {
    try {
      if (descriptor !== undefined) closeSync(descriptor)
    } finally {
      rmSync(temporary, { force: true })
    }
  }
}

export async function applyFiles(
  root: string,
  originals: ReleaseFiles,
  replacements: ReleaseFiles,
  replace: CheckedFileReplace = atomicReplace,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  for (const [relative, original] of originals) {
    if (!readReleaseFile(join(root, relative)).equals(original)) {
      throw new ReleaseError(`file changed during version preparation: ${relative}`)
    }
  }
  for (const relative of replacements.keys()) {
    if (!originals.has(relative)) throw new ReleaseError(`replacement has no checked original: ${relative}`)
  }
  const attempted: { relative: string; original: Buffer; replacement: Buffer }[] = []
  try {
    for (const [relative, replacement] of replacements) {
      const original = originals.get(relative)
      if (!original) throw new ReleaseError(`replacement has no checked original: ${relative}`)
      attempted.push({ relative, original, replacement })
      replace(join(root, relative), replacement, original)
      await setImmediate()
      signal?.throwIfAborted()
    }
  } catch (failure) {
    const unrestored: string[] = []
    for (const { relative, original, replacement } of attempted.reverse()) {
      const path = join(root, relative)
      try {
        const current = readReleaseFile(path)
        if (current.equals(original)) continue
        if (!current.equals(replacement)) throw new ReleaseError('concurrent edit preserved')
        replace(path, original, replacement)
      } catch (error) {
        unrestored.push(`${relative}: ${errorMessage(error)}`)
      }
    }
    if (unrestored.length) {
      throw new ReleaseError(
        `version update failed: ${
          failure instanceof Error ? failure.message : String(failure)
        }; inspect files that could not be restored:\n${unrestored.join('\n')}`,
        { cause: failure },
      )
    }
    throw failure
  }
}

export async function withUpdateLock<T>(root: string, operation: () => T | Promise<T>): Promise<T> {
  const gitPath = run(root, ['git', 'rev-parse', '--git-path', 'tola-version.lock'])
  const path = isAbsolute(gitPath) ? gitPath : join(root, gitPath)
  let descriptor: number
  try {
    descriptor = openSync(path, 'wx', 0o600)
  } catch (error) {
    if (error instanceof Error && 'code' in error && error.code === 'EEXIST') {
      throw new ReleaseError(
        `another version update owns ${path}; after an interrupted process, inspect the version files before removing this lock`,
        { cause: error },
      )
    }
    throw error
  }
  try {
    try {
      writeFileSync(descriptor, `${process.pid}\n`)
    } finally {
      closeSync(descriptor)
    }
    return await operation()
  } finally {
    rmSync(path)
  }
}
