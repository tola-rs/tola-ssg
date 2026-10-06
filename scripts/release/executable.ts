import assert from 'node:assert/strict'
import type { FileHandle } from 'node:fs/promises'
import { chmod } from 'node:fs/promises'
import { arch, platform } from 'node:os'
import { runProcess } from '../process.ts'
import { openRegularFile, readAt } from './packaging-file.ts'
import type { Target } from './targets.ts'

async function checkElf(
  binary: FileHandle,
  size: bigint,
  target: Target,
  signal?: AbortSignal,
): Promise<string> {
  signal?.throwIfAborted()
  const header = await readAt(binary, 0n, 64)
  assert(
    header.subarray(0, 7).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1])) &&
      (header[7] === 0 || header[7] === 3),
    'Linux target requires a little-endian ELF64 System V/Linux binary',
  )
  assert(
    header.readUInt16LE(18) === (target.machine === 'x86_64' ? 62 : 183),
    `ELF machine does not match ${target.machine}`,
  )
  const kind = header.readUInt16LE(16)
  const entry = header.readBigUInt64LE(24)
  assert(
    (kind === 2 || kind === 3) && header.readUInt32LE(20) === 1 && entry > 0n,
    'ELF is not an executable',
  )
  const programOffset = header.readBigUInt64LE(32)
  const programSize = header.readUInt16LE(54)
  const programCount = header.readUInt16LE(56)
  assert(
    header.readUInt16LE(52) === 64 &&
      programSize === 56 &&
      programCount > 0 &&
      programOffset >= 64n &&
      programOffset + BigInt(programSize * programCount) <= size,
    'invalid ELF program headers',
  )
  let executableEntry = false
  for (let index = 0; index < programCount; index++) {
    signal?.throwIfAborted()
    const segment = await readAt(binary, programOffset + BigInt(index * programSize), 56)
    const segmentKind = segment.readUInt32LE(0)
    const flags = segment.readUInt32LE(4)
    const offset = segment.readBigUInt64LE(8)
    const address = segment.readBigUInt64LE(16)
    const fileSize = segment.readBigUInt64LE(32)
    const memorySize = segment.readBigUInt64LE(40)
    assert(offset + fileSize <= size, 'ELF segment extends beyond the binary')
    assert(segmentKind !== 3, 'Linux static binary contains a dynamic interpreter')
    if (segmentKind === 1) {
      assert(fileSize <= memorySize, 'ELF load segment file size exceeds memory size')
      if ((flags & 1) !== 0 && address <= entry && entry < address + memorySize) executableEntry = true
    }
    if (segmentKind === 2) {
      assert(fileSize % 16n === 0n, 'invalid ELF dynamic section')
      for (let position = offset; position < offset + fileSize; position += 16n) {
        signal?.throwIfAborted()
        const dynamic = await readAt(binary, position, 16)
        const tag = dynamic.readBigInt64LE(0)
        if (tag === 0n) break
        assert(tag !== 1n, 'Linux static binary has a shared-library dependency')
      }
    }
  }
  signal?.throwIfAborted()
  assert(executableEntry, 'ELF entry point is not in an executable load segment')
  return `ELF64 ${target.machine}, Linux static`
}

async function machoPath(binary: FileHandle, offset: bigint, size: number, minimum: number): Promise<Buffer> {
  assert(size >= minimum, 'truncated Mach-O path command')
  const nameOffset = (await readAt(binary, offset + 8n, 4)).readUInt32LE(0)
  assert(nameOffset >= minimum && nameOffset < size, 'invalid Mach-O dependency path offset')
  const name = await readAt(binary, offset + BigInt(nameOffset), Math.min(size - nameOffset, 4096))
  const terminator = name.indexOf(0)
  assert(terminator >= 0, 'unterminated or oversized Mach-O dependency path')
  assert(terminator > 0, 'empty Mach-O dependency path')
  return name.subarray(0, terminator)
}

const DYLIB_COMMANDS = new Set([0xc, 0xd, 0x80000018, 0x8000001f, 0x20, 0x80000023])

async function checkMacho(binary: FileHandle, size: bigint, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted()
  const header = await readAt(binary, 0n, 32)
  assert(
    header.readUInt32LE(0) === 0xfeedfacf &&
      header.readInt32LE(4) === 0x0100000c &&
      header.readUInt32LE(12) === 2,
    'Darwin target requires a thin little-endian arm64 Mach-O executable',
  )
  const count = header.readUInt32LE(16)
  const commandSize = header.readUInt32LE(20)
  const end = 32n + BigInt(commandSize)
  assert(count > 0 && count <= Math.floor(commandSize / 8) && end <= size, 'invalid Mach-O commands')
  let macos = false
  let offset = 32n
  for (let index = 0; index < count; index++) {
    signal?.throwIfAborted()
    assert(offset + 8n <= end, 'truncated Mach-O load command')
    const commandHeader = await readAt(binary, offset, 8)
    const command = commandHeader.readUInt32LE(0)
    const length = commandHeader.readUInt32LE(4)
    assert(
      length >= 8 && length % 8 === 0 && offset + BigInt(length) <= end,
      'invalid Mach-O load command size',
    )
    if (command === 0x32) {
      assert(length >= 24, 'truncated Mach-O build version')
      const buildVersion = await readAt(binary, offset + 8n, 16)
      assert(buildVersion.readUInt32LE(0) === 1, 'Mach-O is not built for macOS')
      assert(
        24n + BigInt(buildVersion.readUInt32LE(12)) * 8n === BigInt(length),
        'invalid Mach-O build tool records',
      )
      macos = true
    } else if (command === 0x24) {
      assert(length === 16, 'invalid Mach-O minimum macOS version')
      macos = true
    } else {
      assert(
        command !== 0x25 && command !== 0x2f && command !== 0x30,
        'Mach-O targets a non-macOS Apple platform',
      )
    }
    if (DYLIB_COMMANDS.has(command) || command === 0x8000001c || command === 0xe) {
      const dependency = await machoPath(binary, offset, length, DYLIB_COMMANDS.has(command) ? 24 : 12)
      assert(
        !dependency.includes('/nix/store/'),
        `Darwin binary depends on a Nix store dylib, rpath, or loader: ${
          JSON.stringify(dependency.toString())
        }`,
      )
    }
    offset += BigInt(length)
  }
  signal?.throwIfAborted()
  assert(offset === end && macos, 'Mach-O commands lack valid macOS platform evidence')
  return 'Mach-O arm64, macOS; no Nix store dylib dependencies'
}

async function checkPe(binary: FileHandle, size: bigint, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted()
  const dos = await readAt(binary, 0n, 64)
  assert(dos.subarray(0, 2).toString('ascii') === 'MZ', 'Windows target requires a PE executable')
  const offset = BigInt(dos.readUInt32LE(60))
  assert(offset >= 64n && offset + 24n <= size, 'invalid PE header offset')
  const header = await readAt(binary, offset, 24)
  assert(header.subarray(0, 4).equals(Buffer.from('PE\0\0')), 'invalid PE signature')
  const sections = header.readUInt16LE(6)
  const optionalSize = header.readUInt16LE(20)
  const flags = header.readUInt16LE(22)
  assert(
    header.readUInt16LE(4) === 0x8664 && (flags & 2) !== 0 && (flags & 0x2000) === 0,
    'Windows target requires an x86_64 executable, not a DLL',
  )
  assert(
    sections > 0 && optionalSize >= 112 && offset + 24n + BigInt(optionalSize + sections * 40) <= size,
    'invalid PE optional header or sections',
  )
  const optional = await readAt(binary, offset + 24n, 112)
  assert(optional.readUInt16LE(0) === 0x20b, 'Windows target requires PE32+')
  const entry = BigInt(optional.readUInt32LE(16))
  const subsystem = optional.readUInt16LE(68)
  assert(entry > 0n && (subsystem === 2 || subsystem === 3), 'PE is not a Windows application')
  let executableEntry = false
  for (let index = 0; index < sections; index++) {
    signal?.throwIfAborted()
    const section = await readAt(binary, offset + 24n + BigInt(optionalSize + index * 40), 40)
    const virtualSize = section.readUInt32LE(8)
    const address = BigInt(section.readUInt32LE(12))
    const rawSize = section.readUInt32LE(16)
    const rawOffset = BigInt(section.readUInt32LE(20))
    assert(rawOffset + BigInt(rawSize) <= size, 'PE section extends beyond the binary')
    if (
      (section.readUInt32LE(36) & 0x20000000) !== 0 &&
      address <= entry &&
      entry < address + BigInt(Math.max(virtualSize, rawSize))
    ) {
      executableEntry = true
    }
  }
  signal?.throwIfAborted()
  assert(executableEntry, 'PE entry point is not in an executable section')
  return 'PE32+ x86_64, Windows'
}

export async function inspectExecutable(path: string, target: Target, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted()
  const binary = await openRegularFile(path)
  try {
    switch (target.system) {
      case 'linux':
        return await checkElf(binary.handle, binary.size, target, signal)
      case 'darwin':
        return await checkMacho(binary.handle, binary.size, signal)
      case 'windows':
        return await checkPe(binary.handle, binary.size, signal)
    }
  } finally {
    await binary.handle.close()
  }
}

export async function verifyBinary(
  binary: string,
  target: Target,
  version: string,
  signal?: AbortSignal,
): Promise<string> {
  signal?.throwIfAborted()
  const evidence = await inspectExecutable(binary, target, signal)
  signal?.throwIfAborted()
  const hostSystem = platform() === 'win32' ? 'windows' : platform()
  const hostMachine = arch() === 'arm64' ? 'aarch64' : arch() === 'x64' ? 'x86_64' : arch()
  if (target.system !== hostSystem || target.machine !== hostMachine) {
    return `${evidence}; cross-target execution not performed (host ${hostSystem}/${hostMachine}, target ${target.system}/${target.machine})`
  }
  await chmod(binary, 0o700)
  const result = await runProcess(binary, ['--version'], {
    timeoutMs: 30_000,
    ...(signal === undefined ? {} : { signal }),
  })
  signal?.throwIfAborted()
  assert(!result.timedOut, 'native --version timed out after 30 seconds')
  assert(result.exitCode === 0, `native --version failed (${result.exitCode}): ${result.stderr.trim()}`)
  const expected = `tola ${version}`
  assert(
    result.stdout.trim() === expected,
    `expected ${JSON.stringify(expected)}, got ${JSON.stringify(result.stdout.trim())}`,
  )
  return `${evidence}; native --version verified: ${version}`
}
