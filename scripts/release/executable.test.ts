import { expect } from '@std/expect'
import { afterEach, beforeEach, test } from '@std/testing/bdd'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { inspectExecutable } from './executable.ts'
import { type Target, TARGETS } from './targets.ts'
import { elfExecutable, machoExecutable, peExecutable } from './testing/executables.ts'

let directory: string
beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), 'tola-executable-test-'))
})
afterEach(async () => {
  await rm(directory, { recursive: true, force: true })
})

async function inspect(bytes: Buffer, target: Target): Promise<string> {
  const path = join(directory, target.binary)
  await writeFile(path, bytes)
  return inspectExecutable(path, target)
}

const linux = TARGETS['x86_64-unknown-linux-musl']
const darwin = TARGETS['aarch64-apple-darwin']
const windows = TARGETS['x86_64-pc-windows-msvc']

test('ELF entry ranges retain every bit above the JavaScript safe integer boundary', async () => {
  const bytes = elfExecutable()
  const address = 1n << 53n
  bytes.writeBigUInt64LE(address, 80)
  bytes.writeBigUInt64LE(1n, 96)
  bytes.writeBigUInt64LE(1n, 104)
  bytes.writeBigUInt64LE(address, 24)
  expect(await inspect(bytes, linux)).toContain('ELF64 x86_64')
  bytes.writeBigUInt64LE(address + 1n, 24)
  await expect(inspect(bytes, linux)).rejects.toThrow('entry point')
})

test('ELF program and segment file offsets are checked before narrowing', async () => {
  const bytes = elfExecutable()
  bytes.writeBigUInt64LE((1n << 53n) + 1n, 32)
  await expect(inspect(bytes, linux)).rejects.toThrow('program headers')
  bytes.writeBigUInt64LE(64n, 32)
  bytes.writeBigUInt64LE((1n << 64n) - 1n, 72)
  await expect(inspect(bytes, linux)).rejects.toThrow('beyond the binary')
})

test('Linux static archives cannot smuggle an interpreter or shared-library dependency', async () => {
  const interpreter = elfExecutable()
  interpreter.writeUInt32LE(3, 64)
  await expect(inspect(interpreter, linux)).rejects.toThrow('dynamic interpreter')
  const dependency = Buffer.concat([elfExecutable(), Buffer.alloc(72)])
  dependency.writeUInt16LE(2, 56)
  dependency.writeUInt32LE(2, 120)
  dependency.writeBigUInt64LE(184n, 128)
  dependency.writeBigUInt64LE(16n, 152)
  dependency.writeBigInt64LE(1n, 184)
  await expect(inspect(dependency, linux)).rejects.toThrow('shared-library dependency')
})

test('ELF rejects wrong architecture and impossible executable load segments', async () => {
  await expect(inspect(elfExecutable('aarch64'), linux)).rejects.toThrow('machine')
  const bytes = elfExecutable()
  bytes.writeBigUInt64LE(127n, 104)
  await expect(inspect(bytes, linux)).rejects.toThrow('memory size')
})

test('Mach-O accepts system dependencies but rejects Nix store rpaths and dylibs', async () => {
  expect(
    await inspect(machoExecutable({ command: 0xc, path: '/usr/lib/libSystem.B.dylib' }), darwin),
  ).toContain('macOS')
  await expect(
    inspect(machoExecutable({ command: 0xc, path: '/nix/store/package/lib/libSystem.dylib' }), darwin),
  ).rejects.toThrow('Nix store')
  await expect(
    inspect(machoExecutable({ command: 0x8000001c, path: '/nix/store/package/lib' }), darwin),
  ).rejects.toThrow('Nix store')
})

test('Mach-O requires complete macOS platform and load-command evidence', async () => {
  const bytes = machoExecutable()
  bytes.writeUInt32LE(2, 40)
  await expect(inspect(bytes, darwin)).rejects.toThrow('not built for macOS')
  bytes.writeUInt32LE(1, 40)
  bytes.writeUInt32LE(1, 52)
  await expect(inspect(bytes, darwin)).rejects.toThrow('build tool records')
  bytes.writeUInt32LE(0, 52)
  bytes.writeUInt32LE(23, 36)
  await expect(inspect(bytes, darwin)).rejects.toThrow('load command size')
})

test('Mach-O dependency paths must terminate within their command', async () => {
  const bytes = machoExecutable({ command: 0xe, path: '/usr/lib/dyld' })
  bytes.fill(0x61, 68)
  await expect(inspect(bytes, darwin)).rejects.toThrow('unterminated')
})

test('PE section bounds and entry-point end are exclusive', async () => {
  const bytes = peExecutable()
  expect(await inspect(bytes, windows)).toContain('PE32+ x86_64')
  bytes.writeUInt32LE(0x1010, 104)
  await expect(inspect(bytes, windows)).rejects.toThrow('entry point')
  bytes.writeUInt32LE(0x1000, 104)
  bytes.writeUInt32LE(241, 220)
  await expect(inspect(bytes, windows)).rejects.toThrow('beyond the binary')
})

test('PE cannot substitute a DLL or a truncated optional header', async () => {
  const bytes = peExecutable()
  bytes.writeUInt16LE(0x2002, 86)
  await expect(inspect(bytes, windows)).rejects.toThrow('not a DLL')
  bytes.writeUInt16LE(2, 86)
  bytes.writeUInt16LE(111, 84)
  await expect(inspect(bytes, windows)).rejects.toThrow('optional header')
})
