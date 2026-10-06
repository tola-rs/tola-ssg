import { createHash } from 'node:crypto'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import type { Readable } from 'node:stream'
import { constants as zlibConstants, gunzipSync, zstdCompressSync } from 'node:zlib'
import { extract } from 'tar-stream'
import { REPOSITORY_ROOT } from '../paths.ts'

const HELP = `Regenerate the fonts this repository carries from the pinned typst-assets release.

Usage:
  deno run --allow-read --allow-write --allow-net fonts/index.ts [--check]

The typst-assets version and checksum are read from Cargo.lock, so the carried fonts always
match the release the workspace builds against. The script downloads that release's crate,
verifies its checksum, and rewrites:

  crates/tola-typst/assets/embedded-fonts.bin   the fonts, compressed behind one index
  crates/tola-typst/NOTICE                typst-assets' notice, verbatim
  crates/tola-typst/licenses/README.md    provenance: source, checksum, carried files

With --check nothing is written; the release is compared against what is committed and any
difference fails.
`

/** The text fonts the crate carries, in the order the container lists them. */
export const CARRIED_FONTS = [
  'LibertinusSerif-Regular.otf',
  'LibertinusSerif-Bold.otf',
  'LibertinusSerif-Italic.otf',
  'LibertinusSerif-BoldItalic.otf',
  'LibertinusSerif-Semibold.otf',
  'LibertinusSerif-SemiboldItalic.otf',
  'NewCMMath-Bold.otf',
  'NewCMMath-Book.otf',
  'NewCMMath-Regular.otf',
  'NewCM10-Regular.otf',
  'NewCM10-Bold.otf',
  'NewCM10-Italic.otf',
  'NewCM10-BoldItalic.otf',
  'DejaVuSansMono-Bold.ttf',
  'DejaVuSansMono-BoldOblique.ttf',
  'DejaVuSansMono-Oblique.ttf',
  'DejaVuSansMono.ttf',
] as const

/** The zstd frame's level: the script runs once, so it may take its time for fewer bytes. */
const COMPRESSION_LEVEL = 19

const CRATE_ROOT = resolve(REPOSITORY_ROOT, 'crates/tola-typst')
const CONTAINER = resolve(CRATE_ROOT, 'assets/embedded-fonts.bin')
const NOTICE = resolve(CRATE_ROOT, 'NOTICE')
const PROVENANCE = resolve(CRATE_ROOT, 'licenses/README.md')

/** One carried font: its upstream bytes, and what they hash to. */
export interface Carried {
  readonly name: string
  readonly sha256: string
  readonly bytes: Uint8Array
}

/** The member name tar-stream presents for one archive entry. */
interface TarEntry {
  readonly name: string
}

/** The typst-assets version and checksum the workspace locks. */
export function pinned(lock: string): { version: string; sha256: string } {
  const entry = lock.match(
    /\[\[package\]\]\nname = "typst-assets"\nversion = "(?<version>[^"]+)"\n[^\n]*\nchecksum = "(?<sha256>[0-9a-f]{64})"/,
  )
  const version = entry?.groups?.version
  const sha256 = entry?.groups?.sha256
  if (version === undefined || sha256 === undefined) {
    throw new Error('Cargo.lock names no locked typst-assets release')
  }
  return { version, sha256 }
}

/** One stream's bytes, once it ends. */
function collect(stream: Readable): Promise<Uint8Array> {
  const { promise, resolve, reject } = Promise.withResolvers<Uint8Array>()
  const chunks: Buffer[] = []
  stream.on('data', (chunk: Buffer) => chunks.push(chunk))
  stream.on('end', () => resolve(Buffer.concat(chunks)))
  stream.on('error', reject)
  return promise
}

/** Every member of a gzipped tar, by its full name. */
async function untar(archive: Uint8Array): Promise<Map<string, Uint8Array>> {
  const members = new Map<string, Uint8Array>()
  const extractor = extract()
  const { promise, resolve, reject } = Promise.withResolvers<void>()
  extractor.on('finish', () => resolve())
  extractor.on('error', reject)
  extractor.on('entry', (header: TarEntry, stream: Readable, next: () => void) => {
    collect(stream).then((bytes) => {
      members.set(header.name, bytes)
      next()
    }, reject)
  })
  extractor.end(gunzipSync(archive))
  await promise
  return members
}

/**
 * The container: an index naming each carried font's offset and length, then one zstd frame
 * of their bytes. A reader takes the index and decodes the frame on its first lookup.
 */
export function container(fonts: readonly Carried[]): Uint8Array {
  const names = new TextEncoder()
  let header = 8 + 4
  for (const font of fonts) header += 1 + names.encode(font.name).length + 8
  const bytes = new Uint8Array(header)
  const view = new DataView(bytes.buffer)
  bytes.set(names.encode('TOLAFNT1'), 0)
  view.setUint32(8, fonts.length, true)
  let at = 12
  let offset = 0
  for (const font of fonts) {
    const name = names.encode(font.name)
    view.setUint8(at, name.length)
    at += 1
    bytes.set(name, at)
    at += name.length
    view.setUint32(at, offset, true)
    view.setUint32(at + 4, font.bytes.length, true)
    at += 8
    offset += font.bytes.length
  }
  const payload = new Uint8Array(offset)
  let written = 0
  for (const font of fonts) {
    payload.set(font.bytes, written)
    written += font.bytes.length
  }
  const compressed = zstdCompressSync(payload, {
    params: { [zlibConstants.ZSTD_c_compressionLevel]: COMPRESSION_LEVEL },
  })
  const framed = new Uint8Array(bytes.length + compressed.length)
  framed.set(bytes, 0)
  framed.set(compressed, bytes.length)
  return framed
}

/** The provenance file: where the carried fonts come from, and what they hash to. */
function provenance(version: string, checksum: string, fonts: readonly Carried[]): string {
  const lines = [
    '# Fonts',
    '',
    '`tola-typst` includes these fonts when its `embed-fonts` feature is enabled. They are',
    `redistributed unmodified from [typst-assets](https://github.com/typst/typst-assets)`,
    `${version}, the release the workspace locks with checksum \`${checksum}\`. The crate's`,
    "`NOTICE` is that release's notice.",
    '',
    'The fonts are embedded compressed in `assets/embedded-fonts.bin`, behind an index naming each',
    'one; a build reads the file and never fetches it. Regenerate with `just scripts::fonts`,',
    'which also fails when the embedded fonts stop matching this release.',
    '',
    '| File | Bytes | SHA-256 |',
    '| --- | ---: | --- |',
  ]
  for (const font of fonts) {
    lines.push(`| ${font.name} | ${font.bytes.length} | ${font.sha256} |`)
  }
  lines.push('')
  return lines.join('\n')
}

/** Whether `path` already holds exactly `bytes`. */
function holds(path: string, bytes: Uint8Array): boolean {
  try {
    return Buffer.from(readFileSync(path)).equals(Buffer.from(bytes))
  } catch {
    return false
  }
}

async function main(): Promise<void> {
  const check = Deno.args.includes('--check')
  const { version, sha256: checksum } = pinned(
    readFileSync(resolve(REPOSITORY_ROOT, 'Cargo.lock'), 'utf8'),
  )
  const url = `https://static.crates.io/crates/typst-assets/typst-assets-${version}.crate`
  const response = await fetch(url)
  if (!response.ok) throw new Error(`${url} answered ${response.status}`)
  const archive = new Uint8Array(await response.arrayBuffer())
  const observed = createHash('sha256').update(archive).digest('hex')
  if (observed !== checksum) {
    throw new Error(`typst-assets ${version} hashes to ${observed}, not the locked ${checksum}`)
  }

  const members = await untar(archive)
  const prefix = `typst-assets-${version}/`
  const fonts = CARRIED_FONTS.map((name) => {
    const bytes = members.get(`${prefix}files/fonts/${name}`)
    if (bytes === undefined) throw new Error(`typst-assets ${version} carries no ${name}`)
    return { name, sha256: createHash('sha256').update(bytes).digest('hex'), bytes }
  })
  const notice = members.get(`${prefix}NOTICE`)
  if (notice === undefined) throw new Error(`typst-assets ${version} carries no NOTICE`)

  const writes = [
    [CONTAINER, container(fonts)],
    [NOTICE, notice],
    [PROVENANCE, new TextEncoder().encode(provenance(version, checksum, fonts))],
  ] as const

  if (check) {
    const stale = writes.filter(([path, bytes]) => !holds(path, bytes))
      .map(([path]) => path.slice(REPOSITORY_ROOT.length + 1))
    if (stale.length > 0) {
      throw new Error(`the carried fonts are stale, rerun \`just scripts::fonts\`: ${stale.join(', ')}`)
    }
    console.log(`the carried fonts are current with typst-assets ${version}`)
    return
  }
  for (const [path, bytes] of writes) {
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, bytes)
    console.log(`${path.slice(REPOSITORY_ROOT.length + 1)}  ${bytes.length} bytes`)
  }
  console.log(`wrote ${fonts.length} fonts from typst-assets ${version}`)
}

if (import.meta.main) {
  if (Deno.args.includes('--help') || Deno.args.includes('-h')) console.log(HELP)
  else await main()
}
