import assert from 'node:assert/strict'
import { createReadStream, createWriteStream } from 'node:fs'
import { Transform, type TransformCallback } from 'node:stream'
import { finished, pipeline } from 'node:stream/promises'
import { createGunzip, createGzip } from 'node:zlib'
import { extract, type Headers, pack } from 'tar-stream'
import { COPY_SIZE, exactFileNumber, openRegularFile, type OpenReleaseFile } from './packaging-file.ts'
import type { Target } from './targets.ts'

const tarTextDecoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })

function tarInteger(bytes: Buffer): bigint {
  const first = bytes[0]
  if (first === 0x80 || first === 0xff) {
    let value = 0n
    for (const byte of bytes.subarray(1)) value = value * 256n + BigInt(byte)
    return first === 0xff ? value - (1n << BigInt((bytes.length - 1) * 8)) : value
  }
  const end = bytes.indexOf(0)
  const digits = bytes
    .subarray(0, end < 0 ? bytes.length : end)
    .toString('latin1')
    .trim()
  assert(/^[+-]?[0-7]*$/.test(digits), 'invalid tar numeric field')
  if (digits === '') return 0n
  const negative = digits.startsWith('-')
  const magnitude = digits.startsWith('-') || digits.startsWith('+') ? digits.slice(1) : digits
  assert(magnitude.length > 0, 'invalid tar numeric field')
  return BigInt(`0o${magnitude}`) * (negative ? -1n : 1n)
}

function tarName(bytes: Buffer): string {
  const end = bytes.indexOf(0)
  return tarTextDecoder.decode(bytes.subarray(0, end < 0 ? bytes.length : end))
}

function inspectTarHeader(header: Buffer, target: Target): bigint {
  const recordedChecksum = tarInteger(header.subarray(148, 156))
  let unsignedChecksum = 8 * 32
  let signedChecksum = unsignedChecksum
  for (let index = 0; index < header.length; index++) {
    if (index >= 148 && index < 156) continue
    const byte = header.readUInt8(index)
    unsignedChecksum += byte
    signedChecksum += byte >= 128 ? byte - 256 : byte
  }
  assert(
    recordedChecksum === BigInt(unsignedChecksum) || recordedChecksum === BigInt(signedChecksum),
    'invalid tar header checksum',
  )
  const name = tarName(header.subarray(0, 100))
  const prefix = tarName(header.subarray(345, 500))
  assert(name === target.binary && prefix === '', 'archive binary must be at its root')
  assert(
    (header[156] === 0 || header[156] === 48) && tarName(header.subarray(157, 257)) === '',
    'archive must contain one ordinary regular file, not a link or extension',
  )
  const mode = tarInteger(header.subarray(100, 108))
  assert(
    (mode & 0o111n) !== 0n && (mode & 0o7000n) === 0n,
    'archive binary must be executable without special permission bits',
  )
  const size = tarInteger(header.subarray(124, 136))
  assert(size > 0n, 'archive contains an empty binary')
  for (
    const [start, length] of [
      [108, 8],
      [116, 8],
      [136, 12],
      [329, 8],
      [337, 8],
    ] as const
  ) {
    tarInteger(header.subarray(start, start + length))
  }
  for (const start of [265, 297]) tarName(header.subarray(start, start + 32))
  return size
}

// Inspect the raw framing: archive iterators hide extension headers and bytes after the end marker.
class ReleaseTarContents extends Transform {
  readonly #header = Buffer.alloc(512)
  readonly #target: Target
  #headerSize = 0
  #remaining = 0n
  #padding = 0
  #trailerSize = 0n

  constructor(target: Target) {
    super()
    this.#target = target
  }

  override _transform(chunk: Buffer, _encoding: BufferEncoding, callback: TransformCallback): void {
    try {
      let offset = 0
      if (this.#headerSize < 512) {
        const length = Math.min(512 - this.#headerSize, chunk.length)
        chunk.copy(this.#header, this.#headerSize, 0, length)
        this.#headerSize += length
        offset += length
        if (this.#headerSize === 512) {
          this.#remaining = inspectTarHeader(this.#header, this.#target)
          this.#padding = Number((512n - (this.#remaining % 512n)) % 512n)
        }
      }
      if (this.#remaining > 0n && offset < chunk.length) {
        const available = chunk.length - offset
        const length = this.#remaining < BigInt(available) ? Number(this.#remaining) : available
        this.push(chunk.subarray(offset, offset + length))
        this.#remaining -= BigInt(length)
        offset += length
      }
      if (this.#remaining === 0n && this.#headerSize === 512) {
        const length = Math.min(this.#padding, chunk.length - offset)
        assert(
          !chunk.subarray(offset, offset + length).some((byte) => byte !== 0),
          'nonzero tar entry padding',
        )
        this.#padding -= length
        offset += length
        assert(
          !chunk.subarray(offset).some((byte) => byte !== 0),
          'unexpected additional tar entry or trailing data',
        )
        this.#trailerSize += BigInt(chunk.length - offset)
      }
      callback()
    } catch (error) {
      callback(error instanceof Error ? error : new Error(String(error)))
    }
  }

  override _flush(callback: TransformCallback): void {
    try {
      assert(
        this.#headerSize === 512 && this.#remaining === 0n && this.#padding === 0,
        'truncated archive or executable',
      )
      assert(
        this.#trailerSize >= 1024n && this.#trailerSize % 512n === 0n,
        'invalid tar end-of-archive padding',
      )
      callback()
    } catch (error) {
      callback(error instanceof Error ? error : new Error(String(error)))
    }
  }
}

async function nextGzipMember(
  source: OpenReleaseFile,
  offset: bigint,
  signal?: AbortSignal,
): Promise<bigint> {
  const buffer = Buffer.allocUnsafe(Math.min(COPY_SIZE, exactFileNumber(source.size - offset)))
  while (offset < source.size) {
    signal?.throwIfAborted()
    const length = Math.min(buffer.length, exactFileNumber(source.size - offset))
    const { bytesRead } = await source.handle.read(buffer, 0, length, exactFileNumber(offset))
    assert(bytesRead > 0, 'truncated compressed archive')
    const nonzero = buffer.subarray(0, bytesRead).findIndex((byte) => byte !== 0)
    if (nonzero !== -1) return offset + BigInt(nonzero)
    offset += BigInt(bytesRead)
  }
  return offset
}

async function* gzipContents(source: OpenReleaseFile, signal?: AbortSignal): AsyncGenerator<Buffer> {
  let offset = 0n
  while (offset < source.size) {
    signal?.throwIfAborted()
    const input = source.handle.createReadStream({
      start: exactFileNumber(offset),
      autoClose: false,
      highWaterMark: COPY_SIZE,
    })
    const decoder = createGunzip()
    const decoding = pipeline(input, decoder, { signal }).then(
      () => ({ status: 'fulfilled' as const, value: undefined }),
      (reason: unknown) => ({ status: 'rejected' as const, reason }),
    )
    let consumed = false
    let completion: PromiseSettledResult<void>
    try {
      const chunks: AsyncIterable<Buffer> = decoder.iterator({ destroyOnReturn: false })
      for await (const chunk of chunks) yield chunk
      consumed = true
    } finally {
      if (!consumed) {
        input.destroy()
        decoder.destroy()
      }
      completion = await decoding
    }
    if (completion.status === 'rejected') throw completion.reason
    assert(
      Number.isSafeInteger(decoder.bytesWritten) && decoder.bytesWritten > 0,
      'invalid compressed archive length',
    )
    offset += BigInt(decoder.bytesWritten)
    assert(offset <= source.size, 'gzip decoder read beyond the archive')
    // Native gunzip can stop at zero padding; every later member must still be decoded and checked.
    if (offset < source.size) offset = await nextGzipMember(source, offset, signal)
  }
}

export async function unpackTar(
  archive: string,
  binary: string,
  target: Target,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  const source = await openRegularFile(archive)
  try {
    signal?.throwIfAborted()
    await pipeline(
      gzipContents(source, signal),
      new ReleaseTarContents(target),
      createWriteStream(binary, { flags: 'wx', mode: 0o600 }),
      { signal },
    )
  } finally {
    await source.handle.close()
  }
}

export async function createTarArchive(
  sourcePath: string,
  archive: string,
  target: Target,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  const source = await openRegularFile(sourcePath)
  try {
    signal?.throwIfAborted()
    assert(source.size > 0n, 'archive contains an empty binary')
    assert(source.size <= 0o77777777777n, 'binary exceeds USTAR size limit')
    const tar = pack()
    const entry = tar.entry({
      name: target.binary,
      type: 'file',
      size: exactFileNumber(source.size),
      mode: 0o755,
      uid: 0,
      gid: 0,
      mtime: new Date(0),
      uname: '',
      gname: '',
    })
    tar.finalize()
    const outcomes = await Promise.allSettled([
      pipeline(tar, createGzip({ level: 6 }), createWriteStream(archive, { flags: 'wx', mode: 0o600 }), {
        signal,
      }),
      pipeline(source.handle.createReadStream({ autoClose: false, highWaterMark: COPY_SIZE }), entry, {
        signal,
      }),
    ])
    for (const outcome of outcomes) if (outcome.status === 'rejected') throw outcome.reason
  } finally {
    await source.handle.close()
  }
}

/**
 * Write the license material as one archive, so a release carries it as a single asset.
 *
 * The entries are written in the order given with a fixed modification time, so the same
 * material always produces the same bytes.
 */
export async function createLicenseArchive(
  entries: readonly { readonly name: string; readonly contents: Buffer }[],
  archive: string,
  signal?: AbortSignal,
): Promise<void> {
  const packer = pack()
  const writing = pipeline(
    packer,
    createGzip(),
    createWriteStream(archive, { mode: 0o600 }),
    ...(signal === undefined ? [] : [{ signal }]),
  )
  for (const entry of entries) {
    signal?.throwIfAborted()
    const file = packer.entry({
      name: entry.name,
      type: 'file',
      size: entry.contents.length,
      mode: 0o644,
      uid: 0,
      gid: 0,
      mtime: new Date(0),
      uname: '',
      gname: '',
    })
    file.end(entry.contents)
    await finished(file)
  }
  packer.finalize()
  await writing
  signal?.throwIfAborted()
}

/** The member names one license archive carries, in the order it stores them. */
export async function licenseArchiveMembers(
  archive: string,
  signal?: AbortSignal,
): Promise<string[]> {
  const names: string[] = []
  const extractor = extract()
  extractor.on('entry', (header: Headers, stream: NodeJS.ReadableStream, next: () => void) => {
    names.push(header.name)
    stream.on('end', next)
    stream.resume()
  })
  await pipeline(
    createReadStream(archive),
    createGunzip(),
    extractor,
    ...(signal === undefined ? [] : [{ signal }]),
  )
  signal?.throwIfAborted()
  return names
}
