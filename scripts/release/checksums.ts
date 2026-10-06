import assert from 'node:assert/strict'
import { openRegularFile } from './packaging-file.ts'

export type ArchiveDigests = ReadonlyMap<string, string>

export async function checkChecksums(
  path: string,
  digests: ArchiveDigests,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  const source = await openRegularFile(path)
  const recorded = new Map<string, string>()
  function record(line: string): void {
    assert(line.length <= 4096, 'invalid checksums.txt entry')
    const match = /^([0-9a-fA-F]{64}) [ *]([^/\\\r\n]+)\n?$/.exec(line)
    assert(match !== null && match[1] !== undefined && match[2] !== undefined, 'invalid checksums.txt entry')
    const [, digest, name] = match
    assert(digests.has(name) && !recorded.has(name), `unexpected or duplicate checksum: ${name}`)
    recorded.set(name, digest.toLowerCase())
  }
  try {
    let pending = ''
    let carriageReturn = false
    for await (
      const chunk of source.handle.createReadStream({
        autoClose: false,
        highWaterMark: 4096,
        ...(signal === undefined ? {} : { signal }),
      })
    ) {
      signal?.throwIfAborted()
      assert(Buffer.isBuffer(chunk) && !chunk.some((byte) => byte > 127), 'checksums.txt must contain ASCII')
      let text = chunk.toString('ascii')
      if (carriageReturn && text.startsWith('\n')) text = text.slice(1)
      carriageReturn = text.endsWith('\r')
      pending += text.replace(/\r\n?/g, '\n')
      let newline = pending.indexOf('\n')
      while (newline >= 0) {
        record(pending.slice(0, newline + 1))
        pending = pending.slice(newline + 1)
        newline = pending.indexOf('\n')
      }
      assert(pending.length <= 4096, 'invalid checksums.txt entry')
    }
    if (pending !== '') record(pending)
  } finally {
    await source.handle.close()
  }
  signal?.throwIfAborted()
  assert(
    recorded.size === digests.size && [...digests].every(([name, digest]) => recorded.get(name) === digest),
    'checksums.txt does not match the complete verified archive set',
  )
  console.log('Verified checksums.txt')
}

export function checksumContents(digests: ArchiveDigests): string {
  return [...digests]
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
    .map(([name, digest]) => `${digest}  ${name}\n`)
    .join('')
}
