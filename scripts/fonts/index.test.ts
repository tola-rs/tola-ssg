import { expect } from '@std/expect'
import { describe, test } from '@std/testing/bdd'
import { zstdDecompressSync } from 'node:zlib'

import { CARRIED_FONTS, container, pinned } from './index.ts'

describe('carried fonts', () => {
  test('the container names each carried font at its bytes', () => {
    const fonts = [
      { name: 'LibertinusSerif-Regular.otf', sha256: 'a', bytes: new Uint8Array([1, 2, 3]) },
      { name: 'DejaVuSansMono.ttf', sha256: 'b', bytes: new Uint8Array([4, 5]) },
    ] as const
    const bytes = container(fonts)
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    expect(new TextDecoder().decode(bytes.subarray(0, 8))).toBe('TOLAFNT1')
    expect(view.getUint32(8, true)).toBe(fonts.length)

    let at = 12
    const indexed = fonts.map(() => {
      const nameLength = view.getUint8(at)
      const name = new TextDecoder().decode(bytes.subarray(at + 1, at + 1 + nameLength))
      const offset = view.getUint32(at + 1 + nameLength, true)
      const length = view.getUint32(at + 5 + nameLength, true)
      at += 9 + nameLength
      return { name, offset, length }
    })
    expect(indexed).toEqual([
      { name: fonts[0].name, offset: 0, length: 3 },
      { name: fonts[1].name, offset: 3, length: 2 },
    ])

    expect([...zstdDecompressSync(bytes.subarray(at))]).toEqual([1, 2, 3, 4, 5])
  })

  test('the locked release supplies the version and checksum', () => {
    const lock = '[[package]]\nname = "typst-assets"\nversion = "0.15.1"\n' +
      'source = "registry+https://github.com/rust-lang/crates.io-index"\n' +
      `checksum = "${'0'.repeat(64)}"\n`
    expect(pinned(lock)).toEqual({ version: '0.15.1', sha256: '0'.repeat(64) })
  })

  test('a lock without typst-assets is refused', () => {
    expect(() => pinned('')).toThrow('names no locked typst-assets release')
  })

  test('the carried list holds each font once', () => {
    expect(new Set(CARRIED_FONTS).size).toBe(CARRIED_FONTS.length)
  })
})
