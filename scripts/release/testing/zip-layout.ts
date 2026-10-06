import assert from 'node:assert/strict'

export function centralOffset(zip: Buffer): number {
  return zip.readUInt32LE(zip.length - 6)
}

export function zip64Archive(zip: Buffer): Buffer {
  const directoryOffset = centralOffset(zip)
  const nameSize = zip.readUInt16LE(26)
  assert(zip.readUInt16LE(28) === 0 && zip.readUInt16LE(directoryOffset + 30) === 0)
  const local = Buffer.from(zip.subarray(0, 30 + nameSize))
  const compressedSize = zip.readUInt32LE(18)
  const size = zip.readUInt32LE(22)
  const extra = Buffer.alloc(20)
  extra.writeUInt16LE(1, 0)
  extra.writeUInt16LE(16, 2)
  extra.writeBigUInt64LE(BigInt(size), 4)
  extra.writeBigUInt64LE(BigInt(compressedSize), 12)
  local.writeUInt16LE(45, 4)
  local.writeUInt32LE(0xffffffff, 18)
  local.writeUInt32LE(0xffffffff, 22)
  local.writeUInt16LE(extra.length, 28)
  const body = zip.subarray(30 + nameSize, directoryOffset)
  const central = Buffer.from(zip.subarray(directoryOffset, zip.length - 22))
  central.writeUInt16LE(45, 6)
  central.writeUInt32LE(0xffffffff, 20)
  central.writeUInt32LE(0xffffffff, 24)
  central.writeUInt16LE(extra.length, 30)
  const prefixSize = local.length + extra.length + body.length
  const directorySize = central.length + extra.length
  const zip64 = Buffer.alloc(56)
  zip64.writeUInt32LE(0x06064b50, 0)
  zip64.writeBigUInt64LE(44n, 4)
  zip64.writeUInt16LE(45, 12)
  zip64.writeUInt16LE(45, 14)
  zip64.writeBigUInt64LE(1n, 24)
  zip64.writeBigUInt64LE(1n, 32)
  zip64.writeBigUInt64LE(BigInt(directorySize), 40)
  zip64.writeBigUInt64LE(BigInt(prefixSize), 48)
  const locator = Buffer.alloc(20)
  locator.writeUInt32LE(0x07064b50, 0)
  locator.writeBigUInt64LE(BigInt(prefixSize + directorySize), 8)
  locator.writeUInt32LE(1, 16)
  const end = Buffer.from(zip.subarray(zip.length - 22))
  end.writeUInt32LE(0xffffffff, 12)
  end.writeUInt32LE(0xffffffff, 16)
  return Buffer.concat([local, extra, body, central, extra, zip64, locator, end])
}

export function storedZip(zip: Buffer, binary: Buffer): Buffer {
  const directoryOffset = centralOffset(zip)
  const local = Buffer.from(zip.subarray(0, 30 + zip.readUInt16LE(26) + zip.readUInt16LE(28)))
  local.writeUInt16LE(0, 8)
  local.writeUInt32LE(binary.length, 18)
  const central = Buffer.from(zip.subarray(directoryOffset, zip.length - 22))
  central.writeUInt16LE(0, 10)
  central.writeUInt32LE(binary.length, 20)
  const end = Buffer.from(zip.subarray(zip.length - 22))
  end.writeUInt32LE(local.length + binary.length, 16)
  return Buffer.concat([local, binary, central, end])
}

export function zipWithLocalExtra(zip: Buffer, extra: Buffer): Buffer {
  assert(zip.readUInt16LE(28) === 0)
  const insertAt = 30 + zip.readUInt16LE(26)
  const result = Buffer.concat([zip.subarray(0, insertAt), extra, zip.subarray(insertAt)])
  result.writeUInt16LE(extra.length, 28)
  result.writeUInt32LE(centralOffset(zip) + extra.length, result.length - 6)
  return result
}
