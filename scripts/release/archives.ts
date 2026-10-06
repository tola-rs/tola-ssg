import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import { Writable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { verifyBinary } from './executable.ts'
import { COPY_SIZE, openRegularFile, regularFile } from './packaging-file.ts'
import { createTarArchive, unpackTar } from './tar-archive.ts'
import type { Target } from './targets.ts'
import { createZipArchive, unpackZip } from './zip-archive.ts'

export async function createArchive(
  source: string,
  archive: string,
  target: Target,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  if (target.system === 'windows') await createZipArchive(source, archive, target, signal)
  else await createTarArchive(source, archive, target, signal)
  signal?.throwIfAborted()
}

export async function archiveDigest(archive: string, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted()
  const source = await openRegularFile(archive)
  try {
    const hash = createHash('sha256')
    await pipeline(
      source.handle.createReadStream({ autoClose: false, highWaterMark: COPY_SIZE }),
      new Writable({
        write(chunk: Buffer, _encoding, callback) {
          hash.update(chunk)
          callback()
        },
      }),
      signal === undefined ? {} : { signal },
    )
    signal?.throwIfAborted()
    return hash.digest('hex')
  } finally {
    await source.handle.close()
  }
}

export async function verifyArchive(
  archive: string,
  target: Target,
  version: string,
  signal?: AbortSignal,
): Promise<string> {
  signal?.throwIfAborted()
  const before = await regularFile(archive)
  signal?.throwIfAborted()
  const staging = await mkdtemp(join(tmpdir(), 'tola-verify-'))
  let evidence: string
  try {
    const binary = join(staging, target.binary)
    if (target.system === 'windows') await unpackZip(archive, binary, target, signal)
    else await unpackTar(archive, binary, target, signal)
    evidence = await verifyBinary(binary, target, version, signal)
  } finally {
    await rm(staging, { recursive: true, force: true })
  }
  const digest = await archiveDigest(archive, signal)
  const after = await regularFile(archive)
  signal?.throwIfAborted()
  assert(
    before.dev === after.dev &&
      before.ino === after.ino &&
      before.size === after.size &&
      before.mtimeNs === after.mtimeNs &&
      before.ctimeNs === after.ctimeNs,
    'archive changed during verification',
  )
  console.log(`Verified ${basename(archive)}: ${evidence}`)
  return digest
}
