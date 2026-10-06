import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { constants as zlibConstants, gunzipSync, zstdCompressSync } from 'node:zlib'
import { REPOSITORY_ROOT } from '../paths.ts'

const HELP = `Regenerate a pronunciation table from the source file it is derived from.

Usage:
  deno run --allow-read --allow-write --allow-net pronunciations/index.ts <source> <path or url>

Sources
  unihan  Han character pronunciations, from the Unihan database. Pass the
          Unihan_Readings.txt inside https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip
  cedict  Chinese word pronunciations, from CC-CEDICT (CC BY-SA 4.0). Pass
          https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz
  jmdict  Japanese word pronunciations, from JMdict (CC BY-SA 4.0, EDRDG terms). Pass
          https://www.edrdg.org/pub/Nihongo/JMdict_e.gz

A path may be a local file or an http(s) URL, and either may be gzipped. The
generated table is committed compressed, behind a zstd frame a build decodes
without fetching anything.
`

const UNIHAN_URL = 'https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip'
const CEDICT_URL = 'https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz'
const JMDICT_URL = 'https://www.edrdg.org/pub/Nihongo/JMdict_e.gz'
const DATA = resolve(REPOSITORY_ROOT, 'crates/tola-pronunciations/data')

/** The zstd frame's level: the script runs once, so it may take its time for fewer bytes. */
const COMPRESSION_LEVEL = 19

interface Table {
  /** The table's file name below `data/`, without an extension. */
  stem: string
  /** What the table holds, for its own header. */
  title: string
  /** The command that writes it, for the header a later reader follows. */
  command: string
  /** Where the pronunciations come from, and under which licence. */
  notice: readonly string[]
  /** What the source says about itself: its version and its date. */
  provenance: readonly string[]
  /** One `name<TAB>pronunciation` line per entry, sorted by name. */
  entries: readonly (readonly [string, string])[]
}

/** The source text: a local file or a URL, either of which may be gzipped. */
async function readSource(path: string): Promise<string> {
  let raw: Uint8Array<ArrayBuffer>
  if (path.startsWith('http://') || path.startsWith('https://')) {
    const response = await fetch(path)
    if (!response.ok) throw new Error(`${path} answered ${response.status}`)
    raw = new Uint8Array(await response.arrayBuffer())
  } else {
    raw = Uint8Array.from(readFileSync(path))
  }
  const text = path.endsWith('.gz') ? gunzipSync(raw) : raw
  return new TextDecoder().decode(text)
}

/** The version and date lines the Unihan file carries. */
function unihanProvenance(source: string): readonly string[] {
  const lines: string[] = []
  for (const line of source.split('\n')) {
    if (!line.startsWith('#')) break
    const field = line.slice(1).trim()
    if (field.startsWith('Date:') || field.startsWith('Unicode Version')) lines.push(field)
  }
  if (lines.length === 0) throw new Error('the source file does not say which version it is')
  return lines
}

/** The release facts the CC-CEDICT file carries in its `#!` header. */
function cedictProvenance(source: string): readonly string[] {
  const wanted = ['entries', 'publisher', 'license', 'date']
  const lines: string[] = []
  for (const line of source.split('\n')) {
    if (!line.startsWith('#!')) continue
    const separator = line.indexOf('=')
    if (separator < 0) continue
    const name = line.slice(2, separator).trim()
    if (!wanted.includes(name)) continue
    lines.push(`${name}: ${line.slice(separator + 1).trim()}`)
  }
  if (lines.length === 0) throw new Error('the source file does not say which version it is')
  return lines
}

/**
 * Order entries by name, so the table can be searched without an index.
 *
 * The order is by code point, which is the byte order a name's UTF-8 spelling has and the
 * order `tola-pronunciations` compares names in. A UTF-16 unit order is not that order: it
 * sorts an astral character as the surrogate pair that spells it, before U+E000..U+FFFF,
 * while the character's code point sorts after them.
 */
export function sorted(
  entries: readonly (readonly [string, string])[],
): readonly (readonly [string, string])[] {
  return [...entries].sort(([left], [right]) => {
    let leftIndex = 0
    let rightIndex = 0
    while (leftIndex < left.length && rightIndex < right.length) {
      const leftPoint = left.codePointAt(leftIndex) ?? 0
      const rightPoint = right.codePointAt(rightIndex) ?? 0
      if (leftPoint !== rightPoint) return leftPoint < rightPoint ? -1 : 1
      leftIndex += leftPoint > 0xffff ? 2 : 1
      rightIndex += rightPoint > 0xffff ? 2 : 1
    }
    return left.length - leftIndex - (right.length - rightIndex)
  })
}

/** The customary Mandarin pronunciation of one character, without its tone mark. */
function mandarinPronunciation(value: string): string {
  const first = value.trim().split(/\s+/)[0] ?? ''
  const toneless = first
    .normalize('NFD')
    .replace(/\p{Mn}+/gu, '')
    .toLowerCase()
  if (!/^[a-z]+$/.test(toneless)) {
    throw new Error(`the source pronunciation \`${value}\` is not a toneless syllable`)
  }
  return toneless
}

/** Every `kMandarin` pronunciation in the Unihan readings file. */
function unihan(source: string): Table {
  const entries: [string, string][] = []
  for (const line of source.split('\n')) {
    if (line.startsWith('#')) continue
    const [codePoint, field, value] = line.split('\t')
    if (field !== 'kMandarin') continue
    if (codePoint === undefined || value === undefined) {
      throw new Error(`the source line \`${line}\` is not a Unihan entry`)
    }
    entries.push([
      String.fromCodePoint(Number.parseInt(codePoint.slice(2), 16)),
      mandarinPronunciation(value),
    ])
  }
  if (entries.length === 0) throw new Error('the source file names no pronunciation')
  return {
    stem: 'unihan',
    title: 'Han character pronunciations',
    command: 'just scripts::pronunciations unihan <Unihan_Readings.txt>',
    notice: [
      `Source: ${UNIHAN_URL}`,
      'Licence: Unicode License v3, https://www.unicode.org/license.txt',
      'Changes: kMandarin only; the first pronunciation of each value;',
      '         tone marks and the diaeresis in ü removed; lowercased; sorted by code point.',
    ],
    provenance: unihanProvenance(source),
    entries: sorted(entries),
  }
}

/** One word's pronunciation: syllables carry no tone and no punctuation. */
function wordPronunciation(value: string): string | undefined {
  const syllables: string[] = []
  for (const syllable of value.trim().split(/\s+/)) {
    const plain = syllable.replace(/\d/g, '').replaceAll('u:', 'u').replaceAll('v', 'u').toLowerCase()
    if (!/^[a-z]+$/.test(plain)) return undefined
    syllables.push(plain)
  }
  return syllables.length === 0 ? undefined : syllables.join(' ')
}

/** Every word pronunciation in the CC-CEDICT file, in both of its spellings. */
function cedict(source: string): Table {
  // The first entry for a word wins: the dictionary lists a pronunciation per sense,
  // and one name has to hold one pronunciation.
  const pronunciations = new Map<string, string>()
  for (const line of source.split('\n')) {
    if (line.startsWith('#') || line.trim() === '') continue
    const match = /^(?<traditional>\S+) (?<simplified>\S+) \[(?<pinyin>[^\]]+)] \//.exec(line)
    const pinyin = match?.groups?.pinyin
    if (match === null || pinyin === undefined) {
      throw new Error(`the source line \`${line}\` is not a CC-CEDICT entry`)
    }
    const pronunciation = wordPronunciation(pinyin)
    // A single character is the character table's to name, and a word with no Han
    // character is text a reader already reads: `110` is not a synonym for its digits.
    if (pronunciation === undefined) continue
    for (const word of [match.groups?.traditional, match.groups?.simplified]) {
      if (word === undefined || [...word].length < 2 || !/\p{Script=Han}/u.test(word)) continue
      if (!pronunciations.has(word)) pronunciations.set(word, pronunciation)
    }
  }
  if (pronunciations.size === 0) throw new Error('the source file names no word')
  return {
    ...cedictMetadata(cedictProvenance(source)),
    entries: sorted([...pronunciations]),
  }
}

function cedictMetadata(provenance: readonly string[]): Omit<Table, 'entries'> {
  return {
    stem: 'cedict',
    title: 'Chinese word pronunciations',
    command: 'just scripts::pronunciations cedict <cedict_1_0_ts_utf-8_mdbg.txt.gz>',
    notice: [
      `Source: CC-CEDICT, ${CEDICT_URL}`,
      'Licence: Creative Commons Attribution-ShareAlike 4.0 International,',
      '         https://creativecommons.org/licenses/by-sa/4.0/',
      'Copyright (C) 1997, 1998 Paul Andrew Denisowski and the CC-CEDICT contributors.',
      'Changes: both spellings; keys of at least two Unicode scalar values containing Han;',
      '         tone digits removed; u: and v (spellings of ü) folded to u;',
      '         unsupported pronunciations omitted; the first entry for a word wins; sorted by name.',
    ],
    provenance,
  }
}

export function jmdict(source: string): Table {
  // The first entry for a word wins, as in CC-CEDICT: one name holds one pronunciation.
  const pronunciations = new Map<string, string>()
  for (const entry of source.split('<entry>').slice(1)) {
    const block = entry.slice(0, entry.indexOf('</entry>'))
    const kanji = [...block.matchAll(/<keb>(?<word>[^<]+)<\/keb>/g)].flatMap((match) =>
      match.groups?.word === undefined ? [] : [match.groups.word]
    )
    // JMdict restrictions qualify the reb in their own r_ele, not the whole entry.
    for (const match of block.matchAll(/<r_ele>(?<reading>[\s\S]*?)<\/r_ele>/g)) {
      const reading = match.groups?.reading
      if (reading === undefined || /<re_nokanji\b[^>]*>/.test(reading)) continue
      const pronunciation = /<reb>(?<kana>[^<]+)<\/reb>/.exec(reading)?.groups?.kana
      if (pronunciation === undefined || !/^[\u3041-\u309e\u30a1-\u30fe\u30fc]+$/.test(pronunciation)) {
        continue
      }
      const spellings = [...reading.matchAll(/<re_restr>(?<word>[^<]+)<\/re_restr>/g)].flatMap(
        (restriction) => (restriction.groups?.word === undefined ? [] : [restriction.groups.word]),
      )
      for (const word of kanji) {
        if (!/\p{Script=Han}/u.test(word) || pronunciations.has(word)) continue
        if (spellings.length > 0 && !spellings.includes(word)) continue
        pronunciations.set(word, pronunciation)
      }
    }
  }
  if (pronunciations.size === 0) throw new Error('the source file names no word')
  return {
    stem: 'jmdict',
    title: 'Japanese word pronunciations',
    command: 'just scripts::pronunciations jmdict <JMdict_e.gz>',
    notice: [
      `Source: JMdict, ${JMDICT_URL}`,
      'Licence: Creative Commons Attribution-ShareAlike 4.0 International,',
      '         https://creativecommons.org/licenses/by-sa/4.0/, with the terms of',
      '         https://www.edrdg.org/edrdg/licence.html',
      'Copyright © Electronic Dictionary Research and Development Group (EDRDG).',
      'Changes: spellings containing Han use their first supported kana reading,',
      '         respecting re_restr and re_nokanji; spellings are kept as in the source;',
      '         the first entry for a spelling wins; sorted by spelling.',
    ],
    provenance: jmdictProvenance(source),
    entries: sorted([...pronunciations]),
  }
}

/** The release fact the JMdict file carries: the day it was created. */
function jmdictProvenance(source: string): readonly string[] {
  const created = /<!--\s*JMdict created:\s*(?<date>[\d-]+)\s*-->/i.exec(source)?.groups?.date
  if (created === undefined) throw new Error('the source file does not say which version it is')
  return [`Created: ${created}`]
}

function tableHeader(table: Omit<Table, 'entries'>): string {
  return [
    `# ${table.title}`,
    ...table.provenance.map((line) => `# ${line}`),
    ...table.notice.map((line) => `# ${line}`),
    `# Generated by \`${table.command}\`; regenerate it instead of editing it.`,
    '',
  ].join('\n')
}

function write(table: Table): void {
  const path = resolve(DATA, `${table.stem}.zst`)
  const body = table.entries.map(([name, pronunciation]) => `${name}\t${pronunciation}`)
  const text = `${tableHeader(table)}${body.join('\n')}\n`
  writeFileSync(
    path,
    zstdCompressSync(text, {
      params: { [zlibConstants.ZSTD_c_compressionLevel]: COMPRESSION_LEVEL },
    }),
  )
  console.log(`${path}: ${body.length} pronunciations`)
}

export async function main(args: readonly string[] = process.argv.slice(2)): Promise<void> {
  const [source, path, ...rest] = args
  if (source === undefined || path === undefined || rest.length > 0) {
    console.log(HELP)
    if (source !== undefined) process.exitCode = 1
    return
  }
  const text = await readSource(path)
  switch (source) {
    case 'unihan':
      write(unihan(text))
      return
    case 'cedict':
      write(cedict(text))
      return
    case 'jmdict':
      write(jmdict(text))
      return
    default:
      throw new Error(`\`${source}\` is not a source this command generates`)
  }
}

if (import.meta.main) await main()
