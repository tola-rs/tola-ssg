import type { Target } from '../targets.ts'

export function elfExecutable(machine: Target['machine'] = 'x86_64'): Buffer {
  const bytes = Buffer.alloc(128)
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0]).copy(bytes)
  bytes.writeUInt16LE(2, 16)
  bytes.writeUInt16LE(machine === 'x86_64' ? 62 : 183, 18)
  bytes.writeUInt32LE(1, 20)
  bytes.writeBigUInt64LE(0x400078n, 24)
  bytes.writeBigUInt64LE(64n, 32)
  bytes.writeUInt16LE(64, 52)
  bytes.writeUInt16LE(56, 54)
  bytes.writeUInt16LE(1, 56)
  bytes.writeUInt32LE(1, 64)
  bytes.writeUInt32LE(5, 68)
  bytes.writeBigUInt64LE(0x400000n, 80)
  bytes.writeBigUInt64LE(128n, 96)
  bytes.writeBigUInt64LE(128n, 104)
  return bytes
}

export function machoExecutable(dependency?: { readonly command: number; readonly path: string }): Buffer {
  const minimum = dependency?.command === 0x8000001c || dependency?.command === 0xe ? 12 : 24
  const dependencySize = dependency === undefined
    ? 0
    : Math.ceil((minimum + Buffer.byteLength(dependency.path) + 1) / 8) * 8
  const bytes = Buffer.alloc(56 + dependencySize)
  bytes.writeUInt32LE(0xfeedfacf, 0)
  bytes.writeInt32LE(0x0100000c, 4)
  bytes.writeUInt32LE(2, 12)
  bytes.writeUInt32LE(dependency === undefined ? 1 : 2, 16)
  bytes.writeUInt32LE(24 + dependencySize, 20)
  bytes.writeUInt32LE(0x32, 32)
  bytes.writeUInt32LE(24, 36)
  bytes.writeUInt32LE(1, 40)
  if (dependency !== undefined) {
    bytes.writeUInt32LE(dependency.command, 56)
    bytes.writeUInt32LE(dependencySize, 60)
    bytes.writeUInt32LE(minimum, 64)
    bytes.write(dependency.path, 56 + minimum, 'utf8')
  }
  return bytes
}

export function peExecutable(): Buffer {
  const bytes = Buffer.alloc(256)
  bytes.write('MZ')
  bytes.writeUInt32LE(64, 60)
  bytes.write('PE\0\0', 64)
  bytes.writeUInt16LE(0x8664, 68)
  bytes.writeUInt16LE(1, 70)
  bytes.writeUInt16LE(112, 84)
  bytes.writeUInt16LE(2, 86)
  bytes.writeUInt16LE(0x20b, 88)
  bytes.writeUInt32LE(0x1000, 104)
  bytes.writeUInt16LE(3, 156)
  bytes.write('.text', 200)
  bytes.writeUInt32LE(16, 208)
  bytes.writeUInt32LE(0x1000, 212)
  bytes.writeUInt32LE(16, 216)
  bytes.writeUInt32LE(240, 220)
  bytes.writeUInt32LE(0x20000000, 236)
  return bytes
}

export function executableForTarget(target: Target): Buffer {
  switch (target.system) {
    case 'linux':
      return elfExecutable(target.machine)
    case 'darwin':
      return machoExecutable()
    case 'windows':
      return peExecutable()
  }
}
