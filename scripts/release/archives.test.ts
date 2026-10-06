import { expect } from '@std/expect'
import { afterEach, beforeEach, test } from '@std/testing/bdd'
import { chmod, mkdtemp, readFile, rm, symlink, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gunzipSync, gzipSync } from 'node:zlib'
import { archiveDigest, createArchive } from './archives.ts'
import { checkChecksums, checksumContents } from './checksums.ts'
import { inspectExecutable } from './executable.ts'
import { createTarArchive, unpackTar } from './tar-archive.ts'
import { archiveName, TARGET_NAMES, TARGETS } from './targets.ts'
import { executableForTarget, peExecutable } from './testing/executables.ts'
import { centralOffset, storedZip, zip64Archive, zipWithLocalExtra } from './testing/zip-layout.ts'
import { createZipArchive, unpackZip } from './zip-archive.ts'

let directory: string
beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), 'tola-archive-test-'))
})
afterEach(async () => {
  await rm(directory, { recursive: true, force: true })
})

const linuxName = 'x86_64-unknown-linux-musl' as const
const windowsName = 'x86_64-pc-windows-msvc' as const
const version = '1.0.0'
const linux = TARGETS[linuxName]
const windows = TARGETS[windowsName]

async function tarArchive(): Promise<Buffer> {
  const source = join(directory, 'source')
  const archive = join(directory, 'source.tar.gz')
  await writeFile(source, executableForTarget(linux))
  await createTarArchive(source, archive, linux)
  return gunzipSync(await readFile(archive))
}

function checksumTarHeader(tar: Buffer): void {
  tar.fill(32, 148, 156)
  const checksum = tar.subarray(0, 512).reduce((sum, byte) => sum + byte, 0)
  tar.write(`${checksum.toString(8).padStart(6, '0')}\0 `, 148, 'ascii')
}

async function extractTar(tar: Buffer): Promise<void> {
  await extractGzipTar(gzipSync(tar))
}

async function extractGzipTar(compressed: Buffer): Promise<void> {
  const archive = join(directory, 'mutated.tar.gz')
  await writeFile(archive, compressed)
  await unpackTar(archive, join(directory, 'extracted'), linux)
}

async function zipArchive(): Promise<Buffer> {
  const source = join(directory, 'source.exe')
  const archive = join(directory, 'source.zip')
  await writeFile(source, peExecutable())
  await createZipArchive(source, archive, windows)
  return readFile(archive)
}

async function extractZip(zip: Buffer): Promise<void> {
  const archive = join(directory, 'mutated.zip')
  await writeFile(archive, zip)
  await unpackZip(archive, join(directory, 'extracted.exe'), windows)
}

test('all four complete generated archives stream, retain their binary, and form a complete checksum list', async () => {
  const digests = new Map<string, string>()
  for (const name of TARGET_NAMES) {
    const target = TARGETS[name]
    const archive = archiveName(name, version)
    const source = join(directory, `${name}.source`)
    const contents = Buffer.concat([executableForTarget(target), Buffer.alloc(2 * 1024 * 1024 + 17, 0x5a)])
    await writeFile(source, contents, { mode: 0o600 })
    const path = join(directory, archive)
    await createArchive(source, path, target)
    const binary = join(directory, `${name}.extracted`)
    if (target.system === 'windows') await unpackZip(path, binary, target)
    else await unpackTar(path, binary, target)
    expect(await readFile(binary)).toEqual(contents)
    expect(await inspectExecutable(binary, target)).toContain(
      target.system === 'linux' ? 'Linux static' : target.system === 'darwin' ? 'macOS' : 'Windows',
    )
    digests.set(archive, await archiveDigest(path))
  }
  const checksumFile = join(directory, 'checksums.txt')
  await writeFile(checksumFile, checksumContents(digests))
  await checkChecksums(checksumFile, digests)
  const missing = new Map(digests)
  missing.delete(archiveName(windowsName, version))
  await writeFile(checksumFile, checksumContents(missing))
  await expect(checkChecksums(checksumFile, digests)).rejects.toThrow('complete verified archive set')
})

test('archive output is reproducible despite source timestamps and permissions', async () => {
  for (
    const [name, target] of [
      [linuxName, linux],
      [windowsName, windows],
    ] as const
  ) {
    const source = join(directory, `${target.binary}.source`)
    await writeFile(source, executableForTarget(target), { mode: 0o600 })
    const first = join(directory, `first-${archiveName(name, version)}`)
    const second = join(directory, `second-${archiveName(name, version)}`)
    await createArchive(source, first, target)
    await chmod(source, 0o777)
    await utimes(source, new Date(1_000_000), new Date(2_000_000))
    await createArchive(source, second, target)
    expect(await archiveDigest(first)).toBe(await archiveDigest(second))
  }
})

test('tar rejects extension headers and link entries even when their checksums are valid', async () => {
  const tar = await tarArchive()
  tar[156] = 120
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('link or extension')
  await rm(join(directory, 'extracted'), { force: true })
  tar[156] = 50
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('link or extension')
})

test('tar requires exactly one root executable without special permission bits', async () => {
  const tar = await tarArchive()
  tar.fill(0, 0, 100)
  tar.write('nested/tola', 0)
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('at its root')
  await rm(join(directory, 'extracted'), { force: true })
  tar.fill(0, 0, 100)
  tar.write('tola', 0)
  tar.write('0004755\0', 100, 'ascii')
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('special permission bits')
})

test("tar preserves a filename's BOM instead of normalizing it into the allowed binary name", async () => {
  const tar = await tarArchive()
  tar.fill(0, 0, 100)
  tar.write('\uFEFFtola', 0, 'utf8')
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('at its root')
})

test('tar checks path prefixes independently of the header magic', async () => {
  const tar = await tarArchive()
  tar.write('ustar ', 257, 'ascii')
  tar.write('../', 345, 'ascii')
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('at its root')
})

test('tar rejects nonzero entry padding and data after its end marker', async () => {
  const tar = await tarArchive()
  tar[512 + 128] = 1
  await expect(extractTar(tar)).rejects.toThrow('entry padding')
  await rm(join(directory, 'extracted'), { force: true })
  tar[512 + 128] = 0
  await expect(extractTar(Buffer.concat([tar, tar.subarray(0, 512)]))).rejects.toThrow('trailing data')
})

test('tar requires two complete end blocks and a valid compressed trailer', async () => {
  const tar = await tarArchive()
  await expect(extractTar(tar.subarray(0, 1536))).rejects.toThrow('end-of-archive padding')
  await rm(join(directory, 'extracted'), { force: true })
  const compressed = gzipSync(tar)
  const archive = join(directory, 'truncated.tar.gz')
  await writeFile(archive, compressed.subarray(0, compressed.length - 1))
  await expect(unpackTar(archive, join(directory, 'extracted'), linux)).rejects.toThrow()
})

test('gzip padding cannot hide an invalid compressed suffix', async () => {
  const tar = await tarArchive()
  const compressed = Buffer.concat([gzipSync(tar), Buffer.from([0, 0, 0]), Buffer.from('not a gzip member')])
  await expect(extractGzipTar(compressed)).rejects.toThrow()
})

test('gzip members separated by zero padding form one checked tar stream', async () => {
  const tar = await tarArchive()
  const compressed = Buffer.concat([
    gzipSync(tar.subarray(0, 512)),
    Buffer.alloc(3),
    gzipSync(tar.subarray(512)),
    Buffer.alloc(4),
  ])
  await extractGzipTar(compressed)
  expect((await readFile(join(directory, 'extracted'))).equals(executableForTarget(linux))).toBe(true)
})

test('tar never rounds a base-256 size down to a different declared binary length', async () => {
  const tar = await tarArchive()
  tar.fill(0, 124, 136)
  tar[124] = 0x80
  tar.writeBigUInt64BE((1n << 53n) + 1n, 128)
  checksumTarHeader(tar)
  await expect(extractTar(tar)).rejects.toThrow('truncated')
})

test('ZIP accepts complete stored and ZIP64 archives as well as generated deflate archives', async () => {
  const zip = await zipArchive()
  await extractZip(storedZip(zip, peExecutable()))
  expect((await readFile(join(directory, 'extracted.exe'))).equals(peExecutable())).toBe(true)
  await rm(join(directory, 'extracted.exe'))
  await extractZip(zip64Archive(zip))
  expect((await readFile(join(directory, 'extracted.exe'))).equals(peExecutable())).toBe(true)
})

test('ZIP validates streamed CRC instead of trusting consistent local and central records', async () => {
  const zip = await zipArchive()
  const crc = (zip.readUInt32LE(14) ^ 1) >>> 0
  zip.writeUInt32LE(crc, 14)
  zip.writeUInt32LE(crc, centralOffset(zip) + 16)
  await expect(extractZip(zip)).rejects.toThrow()
})

test('ZIP rejects descriptor flags and symbolic-link attributes', async () => {
  const zip = await zipArchive()
  const central = centralOffset(zip)
  const flags = zip.readUInt16LE(6)
  zip.writeUInt16LE(flags | 8, 6)
  zip.writeUInt16LE(flags | 8, central + 8)
  await expect(extractZip(zip)).rejects.toThrow('ZIP flags')
  zip.writeUInt16LE(flags, 6)
  zip.writeUInt16LE(flags, central + 8)
  zip.writeUInt32LE((0o120777 << 16) >>> 0, central + 38)
  await expect(extractZip(zip)).rejects.toThrow('regular file')
})

test('ZIP local names and headers must match its single central record', async () => {
  const zip = await zipArchive()
  zip[30] = 0
  await expect(extractZip(zip)).rejects.toThrow('local filename')
  zip[30] = 116
  zip.writeUInt32LE((zip.readUInt32LE(14) ^ 1) >>> 0, 14)
  await expect(extractZip(zip)).rejects.toThrow('local header')
})

test('ZIP rejects hidden name suffixes and additional trailer or directory bytes', async () => {
  const zip = await zipArchive()
  const central = centralOffset(zip)
  zip[central + 46 + 4] = 0
  await expect(extractZip(zip)).rejects.toThrow('hidden name suffixes')
  zip[central + 46 + 4] = 46
  await expect(extractZip(Buffer.concat([zip, Buffer.from([0])]))).rejects.toThrow('trailer')
  zip.writeUInt32LE(zip.readUInt32LE(zip.length - 10) + 1, zip.length - 10)
  await expect(extractZip(zip)).rejects.toThrow('directory and trailer')
})

test('ZIP extra fields cannot hide unknown records or truncated ZIP64 values', async () => {
  const zip = await zipArchive()
  await expect(extractZip(zipWithLocalExtra(zip, Buffer.from([0x55, 0x54, 0, 0])))).rejects.toThrow(
    'only ZIP64',
  )
  await expect(extractZip(zipWithLocalExtra(zip, Buffer.from([1, 0, 16, 0, 0])))).rejects.toThrow(
    'ZIP extra field',
  )
})

test('ZIP64 local sizes preserve differences above 2^53 and reject unrepresentable decoding', async () => {
  const zip = zip64Archive(await zipArchive())
  const localSizeOffset = 30 + zip.readUInt16LE(26) + 4
  const directory = Number(zip.readBigUInt64LE(zip.length - 22 - 20 - 8))
  const centralSizeOffset = directory + 46 + zip.readUInt16LE(directory + 28) + 4
  const size = 1n << 53n
  zip.writeBigUInt64LE(size, centralSizeOffset)
  zip.writeBigUInt64LE(size + 1n, localSizeOffset)
  await expect(extractZip(zip)).rejects.toThrow('inconsistent ZIP64 local sizes')
  zip.writeBigUInt64LE(size, localSizeOffset)
  await expect(extractZip(zip)).rejects.toThrow('exact runtime integer range')
})

test('ZIP64 locator pointers and record counts must describe the exact trailer', async () => {
  const zip = zip64Archive(await zipArchive())
  const locator = zip.length - 42
  zip.writeBigUInt64LE((1n << 63n) + 1n, locator + 8)
  await expect(extractZip(zip)).rejects.toThrow('ZIP64 locator')
  zip.writeBigUInt64LE(BigInt(zip.length - 98), locator + 8)
  zip.writeBigUInt64LE(2n, zip.length - 98 + 24)
  await expect(extractZip(zip)).rejects.toThrow('ZIP64 directory trailer')
})

test('packaging does not follow source or destination links or overwrite extraction output', async () => {
  const source = join(directory, 'ordinary')
  const linked = join(directory, 'linked')
  await writeFile(source, peExecutable())
  await symlink(source, linked)
  await expect(createArchive(linked, join(directory, 'rejected.zip'), windows)).rejects.toThrow(
    'regular file',
  )
  const destination = join(directory, 'destination.zip')
  await symlink(source, destination)
  await expect(createArchive(source, destination, windows)).rejects.toThrow()
  expect((await readFile(source)).equals(peExecutable())).toBe(true)
  const zip = await zipArchive()
  const extracted = join(directory, 'extracted.exe')
  await writeFile(extracted, 'do not replace')
  await expect(extractZip(zip)).rejects.toThrow()
  expect(await readFile(extracted, 'utf8')).toBe('do not replace')
})

test('checksums reject duplicate entries, mismatches, and path-bearing names', async () => {
  const windowsArchive = archiveName(windowsName, version)
  const digests = new Map([[windowsArchive, 'a'.repeat(64)]])
  const file = join(directory, 'checksums.txt')
  const contents = checksumContents(digests)
  await writeFile(file, contents + contents)
  await expect(checkChecksums(file, digests)).rejects.toThrow('duplicate checksum')
  await writeFile(file, contents.replace('a', 'b'))
  await expect(checkChecksums(file, digests)).rejects.toThrow('complete verified archive set')
  await writeFile(file, `${'a'.repeat(64)}  ../${windowsArchive}\n`)
  await expect(checkChecksums(file, digests)).rejects.toThrow('invalid checksums.txt entry')
})

test('checksum newline and case normalization still rejects duplicate names', async () => {
  const windowsArchive = archiveName(windowsName, version)
  const linuxArchive = archiveName(linuxName, version)
  const digests = new Map([
    [windowsArchive, 'a'.repeat(64)],
    [linuxArchive, 'b'.repeat(64)],
  ])
  const file = join(directory, 'checksums.txt')
  await writeFile(file, `${'A'.repeat(64)} *${windowsArchive}\r\n${'B'.repeat(64)}  ${linuxArchive}\r`)
  await checkChecksums(file, digests)
  await writeFile(
    file,
    `${'A'.repeat(64)} *${windowsArchive}\r\n${'B'.repeat(64)}  ${linuxArchive}\r${
      'a'.repeat(64)
    }  ${windowsArchive}\n`,
  )
  await expect(checkChecksums(file, digests)).rejects.toThrow('duplicate checksum')
})
