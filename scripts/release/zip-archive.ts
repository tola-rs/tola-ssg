import assert from 'node:assert/strict'
import { type FileHandle, mkdtemp, open, rm } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { Reader, type TempStream, ZipReader, ZipWriter } from '@zip.js/zip.js'
import { errorMessage } from '../process.ts'
import { COPY_SIZE, exactFileNumber, openRegularFile, readAt, writeAll } from './packaging-file.ts'
import type { Target } from './targets.ts'
import { inspectZipHeaders } from './zip-headers.ts'

class ReleaseFileReader extends Reader<FileHandle> {
  readonly #file: FileHandle

  constructor(file: FileHandle, size: bigint) {
    super(file)
    this.#file = file
    this.size = exactFileNumber(size)
  }

  override async readUint8Array(index: number, length: number): Promise<Uint8Array> {
    assert(
      Number.isSafeInteger(index) && index >= 0 && Number.isSafeInteger(length) && length >= 0,
      'invalid ZIP read range',
    )
    const contents = await readAt(this.#file, BigInt(index), Math.min(length, Math.max(0, this.size - index)))
    // zip.js relies on Uint8Array.slice copying header bytes, unlike Buffer.slice.
    return new Uint8Array(contents.buffer, contents.byteOffset, contents.byteLength)
  }
}

function fileWritable(file: FileHandle): WritableStream<Uint8Array> {
  return new WritableStream<Uint8Array>({ write: (chunk) => writeAll(file, chunk) })
}

function compressedFileStream(file: FileHandle): TempStream {
  let offset = 0
  return {
    writable: fileWritable(file),
    readable: new ReadableStream<Uint8Array>(
      {
        async pull(controller) {
          const chunk = Buffer.allocUnsafe(COPY_SIZE)
          const { bytesRead } = await file.read(chunk, 0, chunk.length, offset)
          if (bytesRead === 0) {
            controller.close()
          } else {
            offset += bytesRead
            assert(Number.isSafeInteger(offset), 'compressed ZIP size exceeds exact runtime integer range')
            controller.enqueue(chunk.subarray(0, bytesRead))
          }
        },
      },
      { highWaterMark: 0 },
    ),
  }
}

export async function createZipArchive(
  sourcePath: string,
  archive: string,
  target: Target,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  const source = await openRegularFile(sourcePath)
  let destination: FileHandle | undefined
  let compressed: FileHandle | undefined
  let staging: string | undefined
  const failures: unknown[] = []
  try {
    signal?.throwIfAborted()
    assert(source.size > 0n, 'archive contains an empty binary')
    const reader = new ReleaseFileReader(source.handle, source.size)
    destination = await open(archive, 'wx+', 0o600)
    const writer = new ZipWriter(fileWritable(destination), {
      ...(signal === undefined ? {} : { signal }),
      useWebWorkers: false,
      useCompressionStream: false,
      level: 6,
      dataDescriptor: false,
      extendedTimestamp: false,
      supportZip64SplitFile: false,
      lastModDate: new Date('1980-01-01T00:00:00.000Z'),
      rawLastModDate: 0x00210000,
      unixMode: 0o100755,
      async createTempStream() {
        signal?.throwIfAborted()
        assert(staging === undefined, 'release ZIP contains more than one compressed entry')
        // CRC and sizes must precede the body; keep that body on disk, not in a RAM buffer.
        staging = await mkdtemp(join(dirname(archive), '.zip-body-'))
        compressed = await open(join(staging, 'deflate'), 'wx+', 0o600)
        return compressedFileStream(compressed)
      },
    })
    const entry = await writer.add(target.binary, reader)
    assert(entry.compressionMethod === 8, 'ZIP deflate codec is unavailable')
    assert(BigInt(entry.uncompressedSize) === source.size, 'binary changed size while packaging')
    signal?.throwIfAborted()
    await writer.close()
    const size = (await destination.stat({ bigint: true })).size
    const end = await readAt(destination, size - 22n, 22)
    assert(end.readUInt32LE(0) === 0x06054b50, 'ZIP writer produced an invalid trailer')
    // ZIP64 still has exactly one entry; keep the ordinary trailer's count explicit.
    end.writeUInt16LE(1, 8)
    end.writeUInt16LE(1, 10)
    signal?.throwIfAborted()
    await writeAll(destination, end, exactFileNumber(size - 22n))
    await inspectZipHeaders(destination, size, target)
  } catch (error) {
    failures.push(error)
  }
  await closeZipFiles(failures, [source.handle, destination, compressed], staging)
}

export async function unpackZip(
  archive: string,
  binary: string,
  target: Target,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  const source = await openRegularFile(archive)
  let destination: FileHandle | undefined
  let reader: ZipReader<FileHandle> | undefined
  const failures: unknown[] = []
  try {
    signal?.throwIfAborted()
    const expected = await inspectZipHeaders(source.handle, source.size, target)
    reader = new ZipReader(new ReleaseFileReader(source.handle, source.size), {
      useWebWorkers: false,
      strictness: 'strict',
      checkCrc32: true,
      ...(signal === undefined ? {} : { signal }),
    })
    const entries = await reader.getEntries()
    const entry = entries[0]
    assert(
      entries.length === 1 && entry !== undefined && !entry.directory,
      'ZIP archive must contain exactly one ordinary entry',
    )
    assert(
      entry.filename === target.binary &&
        BigInt(entry.uncompressedSize) === expected.size &&
        BigInt(entry.compressedSize) === expected.compressedSize &&
        entry.crc32 === expected.crc32,
      'inconsistent ZIP directory decoding',
    )
    signal?.throwIfAborted()
    destination = await open(binary, 'wx', 0o600)
    const file = destination
    let written = 0n
    await entry.getData(
      new WritableStream<Uint8Array>({
        async write(chunk) {
          signal?.throwIfAborted()
          written += BigInt(chunk.byteLength)
          assert(written <= expected.size, 'ZIP binary size does not match its entry')
          await writeAll(file, chunk)
        },
      }),
      signal === undefined ? {} : { signal },
    )
    assert(written === expected.size, 'ZIP binary size does not match its entry')
  } catch (error) {
    failures.push(error)
  }
  try {
    await reader?.close()
  } catch (error) {
    failures.push(error)
  }
  await closeZipFiles(failures, [source.handle, destination])
}

async function closeZipFiles(
  failures: unknown[],
  files: readonly (FileHandle | undefined)[],
  staging?: string,
): Promise<void> {
  const closed = await Promise.allSettled(files.map((file) => file?.close()))
  for (const completion of closed) {
    if (completion.status === 'rejected') failures.push(completion.reason)
  }
  if (staging !== undefined) {
    try {
      await rm(staging, { recursive: true, force: true })
    } catch (error) {
      failures.push(error)
    }
  }
  if (failures.length === 1) throw failures[0]
  if (failures.length > 1) {
    throw new AggregateError(failures, failures.map(errorMessage).join('\n'))
  }
}
