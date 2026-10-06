import assert from 'node:assert/strict'
import type { FileHandle } from 'node:fs/promises'
import { exactFileNumber, readAt } from './packaging-file.ts'
import type { Target } from './targets.ts'

const UINT32_MAX = 0xffffffff

export interface ReleaseZipEntry {
  readonly compressedSize: bigint
  readonly size: bigint
  readonly crc32: number
  readonly compression: number
  readonly directoryOffset: bigint
}

function zip64Extra(extra: Buffer): Buffer | undefined {
  let zip64: Buffer | undefined
  let offset = 0
  while (offset < extra.length) {
    assert(offset + 4 <= extra.length, 'truncated ZIP extra field')
    const kind = extra.readUInt16LE(offset)
    const size = extra.readUInt16LE(offset + 2)
    assert(
      kind === 1 && zip64 === undefined && size <= extra.length - offset - 4,
      'unexpected ZIP extra field (only ZIP64 is supported)',
    )
    zip64 = extra.subarray(offset + 4, offset + 4 + size)
    offset += 4 + size
  }
  return zip64
}

interface ZipDirectory {
  readonly offset: bigint
  readonly size: bigint
}

async function inspectDirectoryTrailer(raw: FileHandle, archiveSize: bigint): Promise<ZipDirectory> {
  assert(archiveSize >= 22n, 'truncated ZIP trailer')
  const endOffset = archiveSize - 22n
  const end = await readAt(raw, endOffset, 22)
  assert(
    end.readUInt32LE(0) === 0x06054b50 &&
      end.readUInt16LE(4) === 0 &&
      end.readUInt16LE(6) === 0 &&
      end.readUInt16LE(8) === 1 &&
      end.readUInt16LE(10) === 1 &&
      end.readUInt16LE(20) === 0,
    'unexpected ZIP trailer or multiple disks',
  )
  let directorySize = BigInt(end.readUInt32LE(12))
  let directoryOffset = BigInt(end.readUInt32LE(16))
  let trailerStart = endOffset
  if (endOffset >= 20n) {
    const locator = await readAt(raw, endOffset - 20n, 20)
    if (locator.readUInt32LE(0) === 0x07064b50) {
      const zip64Offset = locator.readBigUInt64LE(8)
      assert(
        locator.readUInt32LE(4) === 0 &&
          locator.readUInt32LE(16) === 1 &&
          zip64Offset + 56n === endOffset - 20n,
        'invalid ZIP64 locator or multiple disks',
      )
      const zip64 = await readAt(raw, zip64Offset, 56)
      assert(
        zip64.readUInt32LE(0) === 0x06064b50 &&
          zip64.readBigUInt64LE(4) === 44n &&
          zip64.readUInt32LE(16) === 0 &&
          zip64.readUInt32LE(20) === 0 &&
          zip64.readBigUInt64LE(24) === 1n &&
          zip64.readBigUInt64LE(32) === 1n,
        'invalid ZIP64 directory trailer',
      )
      const size = zip64.readBigUInt64LE(40)
      const offset = zip64.readBigUInt64LE(48)
      assert(
        (directorySize === BigInt(UINT32_MAX) || directorySize === size) &&
          (directoryOffset === BigInt(UINT32_MAX) || directoryOffset === offset),
        'inconsistent ZIP64 directory trailer',
      )
      directorySize = size
      directoryOffset = offset
      trailerStart = zip64Offset
    }
  }
  assert(
    directoryOffset + directorySize === trailerStart && directorySize >= 46n,
    'unexpected data between the ZIP directory and trailer',
  )
  return { offset: directoryOffset, size: directorySize }
}

function centralSizes(
  header: Buffer,
  extra: Buffer | undefined,
): { size: bigint; compressedSize: bigint; localOffset: bigint; disk: bigint } {
  let position = 0
  function resolve(value: number, sentinel: number, width: 4 | 8): bigint {
    if (value !== sentinel) return BigInt(value)
    assert(extra !== undefined && position + width <= extra.length, 'missing or truncated ZIP64 extra field')
    const decoded = width === 8 ? extra.readBigUInt64LE(position) : BigInt(extra.readUInt32LE(position))
    position += width
    return decoded
  }
  const size = resolve(header.readUInt32LE(24), UINT32_MAX, 8)
  const compressedSize = resolve(header.readUInt32LE(20), UINT32_MAX, 8)
  const localOffset = resolve(header.readUInt32LE(42), UINT32_MAX, 8)
  const disk = resolve(header.readUInt16LE(34), 0xffff, 4)
  assert(extra === undefined || position === extra.length, 'unexpected ZIP64 central extra data')
  return { size, compressedSize, localOffset, disk }
}

function inspectLocalSizes(header: Buffer, extra: Buffer | undefined, entry: ReleaseZipEntry): void {
  const compressed = header.readUInt32LE(18)
  const size = header.readUInt32LE(22)
  assert(
    (compressed === UINT32_MAX || BigInt(compressed) === entry.compressedSize) &&
      (size === UINT32_MAX || BigInt(size) === entry.size),
    'inconsistent ZIP local sizes',
  )
  if (compressed === UINT32_MAX || size === UINT32_MAX || extra !== undefined) {
    assert(extra !== undefined && extra.length === 16, 'missing or invalid ZIP64 local sizes')
    assert(
      extra.readBigUInt64LE(0) === entry.size && extra.readBigUInt64LE(8) === entry.compressedSize,
      'inconsistent ZIP64 local sizes',
    )
  }
}

// A release ZIP is one root executable, not a general-purpose container.
export async function inspectZipHeaders(
  raw: FileHandle,
  archiveSize: bigint,
  target: Target,
): Promise<ReleaseZipEntry> {
  const directory = await inspectDirectoryTrailer(raw, archiveSize)
  const central = await readAt(raw, directory.offset, 46)
  assert(central.readUInt32LE(0) === 0x02014b50, 'invalid ZIP central directory')
  const nameSize = central.readUInt16LE(28)
  const extraSize = central.readUInt16LE(30)
  const commentSize = central.readUInt16LE(32)
  assert(
    directory.size === BigInt(46 + nameSize + extraSize + commentSize),
    'ZIP archive must contain exactly one entry without trailing directory data',
  )
  assert(commentSize === 0, 'unexpected ZIP entry comment')
  const name = await readAt(raw, directory.offset + 46n, nameSize)
  assert(
    name.equals(Buffer.from(target.binary, 'ascii')),
    'archive binary must be at its root without hidden name suffixes',
  )
  const attributes = central.readUInt32LE(38)
  const mode = attributes >>> 16
  assert(
    central.readUInt16LE(4) >>> 8 === 3 && (mode & 0o170000) === 0o100000 && (attributes & 0x10) === 0,
    'ZIP entry must be a regular file, not a link or directory',
  )
  assert(
    (mode & 0o111) !== 0 && (mode & 0o7000) === 0,
    'ZIP binary must be executable without special permission bits',
  )
  const flags = central.readUInt16LE(8)
  const compression = central.readUInt16LE(10)
  assert((flags & ~0x800) === 0, 'unexpected ZIP flags, wrapper, disk, or comment')
  assert(compression === 0 || compression === 8, 'unsupported ZIP compression')
  const extra = zip64Extra(await readAt(raw, directory.offset + 46n + BigInt(nameSize), extraSize))
  const sizes = centralSizes(central, extra)
  assert(sizes.disk === 0n && sizes.localOffset === 0n, 'unexpected ZIP flags, wrapper, disk, or comment')
  assert(sizes.size > 0n, 'archive contains an empty binary')
  const entry: ReleaseZipEntry = {
    size: sizes.size,
    compressedSize: sizes.compressedSize,
    crc32: central.readUInt32LE(16),
    compression,
    directoryOffset: directory.offset,
  }
  const local = await readAt(raw, 0n, 30)
  assert(
    local.readUInt32LE(0) === 0x04034b50 &&
      local.readUInt16LE(4) === central.readUInt16LE(6) &&
      local.readUInt16LE(6) === flags &&
      local.readUInt16LE(8) === compression &&
      local.readUInt32LE(10) === central.readUInt32LE(12) &&
      local.readUInt32LE(14) === entry.crc32,
    'inconsistent ZIP local header',
  )
  const localNameSize = local.readUInt16LE(26)
  const localExtraSize = local.readUInt16LE(28)
  assert((await readAt(raw, 30n, localNameSize)).equals(name), 'unexpected ZIP local filename')
  inspectLocalSizes(local, zip64Extra(await readAt(raw, 30n + BigInt(localNameSize), localExtraSize)), entry)
  assert(
    directory.offset === 30n + BigInt(localNameSize + localExtraSize) + entry.compressedSize,
    'unexpected data between the ZIP binary and directory',
  )
  if (compression === 0) {
    assert(entry.size === entry.compressedSize, 'stored ZIP binary size does not match its entry')
  }
  // The codec uses numbers; never let a ZIP64 integer be rounded on that boundary.
  exactFileNumber(archiveSize)
  exactFileNumber(entry.size)
  exactFileNumber(entry.compressedSize)
  return entry
}
