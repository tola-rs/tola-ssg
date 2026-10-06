import assert from 'node:assert/strict'
import { type BigIntStats, constants } from 'node:fs'
import { type FileHandle, lstat, open } from 'node:fs/promises'

export const COPY_SIZE = 1024 * 1024

export async function regularFile(path: string): Promise<BigIntStats> {
  const metadata = await lstat(path, { bigint: true })
  assert(metadata.isFile(), `not a regular file: ${path}`)
  return metadata
}

export async function ordinaryDirectory(path: string): Promise<void> {
  const metadata = await lstat(path)
  assert(metadata.isDirectory() && !metadata.isSymbolicLink(), `not an ordinary directory: ${path}`)
}

export async function ordinaryDestination(path: string): Promise<void> {
  try {
    await regularFile(path)
  } catch (error) {
    if (!(error instanceof Error && 'code' in error && error.code === 'ENOENT')) throw error
  }
}

export interface OpenReleaseFile {
  readonly handle: FileHandle
  readonly size: bigint
}

export async function openRegularFile(path: string): Promise<OpenReleaseFile> {
  const before = await regularFile(path)
  const handle = await open(
    path,
    constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0),
  )
  try {
    const after = await handle.stat({ bigint: true })
    assert(
      after.isFile() && before.dev === after.dev && before.ino === after.ino,
      `file changed while opening: ${path}`,
    )
    return { handle, size: after.size }
  } catch (error) {
    await handle.close()
    throw error
  }
}

export function exactFileNumber(value: bigint): number {
  assert(
    value >= 0n && value <= BigInt(Number.MAX_SAFE_INTEGER),
    'file size or offset exceeds exact runtime integer range',
  )
  return Number(value)
}

export async function readAt(handle: FileHandle, offset: bigint, size: number): Promise<Buffer> {
  assert(Number.isSafeInteger(size) && size >= 0, 'invalid executable or archive read size')
  exactFileNumber(offset + BigInt(size))
  const buffer = Buffer.allocUnsafe(size)
  let consumed = 0
  while (consumed < size) {
    const { bytesRead } = await handle.read(
      buffer,
      consumed,
      size - consumed,
      exactFileNumber(offset + BigInt(consumed)),
    )
    assert(bytesRead > 0, 'truncated archive or executable')
    consumed += bytesRead
  }
  return buffer
}

export async function writeAll(
  handle: FileHandle,
  contents: Uint8Array,
  position: number | null = null,
): Promise<void> {
  let written = 0
  while (written < contents.byteLength) {
    const { bytesWritten } = await handle.write(
      contents,
      written,
      contents.byteLength - written,
      position === null ? null : position + written,
    )
    assert(bytesWritten > 0, 'incomplete release file write')
    written += bytesWritten
  }
}
