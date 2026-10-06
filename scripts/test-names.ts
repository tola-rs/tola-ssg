// A test name is an identifier for the one behaviour a test proves, not a sentence about the code it
// runs: `.agents/skills/trim/SKILL.md` carries the rules, and this refuses the names that break
// them before anyone has to read a failure line.
//
// A rule earns its place only when it names a shape no correct test name has, so every rule here was
// measured against every name in the workspace before it was kept. `test-names.test.ts` holds the
// names on both sides of that line — including the names the suite carried when a rule found them —
// and fails when a rule has no example on either side, or when the skill stops stating it.
//
// The audit reads every test the workspace declares, including the ones it cannot name: a test
// attribute whose function the scanner cannot reach fails the audit instead of skipping the name,
// and a case title the scanner cannot read does the same.

import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { REPOSITORY_ROOT } from './paths.ts'

/** A failure line has room for this many characters and no more: the longest name the suite carries,
 * which only a deliberate edit may raise. */
export const CEILING = 56

/** One shape a test name or case title must never have. */
export interface Shape {
  /** The rule as `skill://trim` states it, and the key its examples are filed under. */
  readonly shape: string
  /** The sentence a failure line carries before the name. */
  readonly message: string
  readonly matches: (name: string) => boolean
}

/** Shapes a behaviour identifier never has. A connective may join two domain values —
 * `query_and_fragment_are_refused` — so only a second connective, a connective spelling prose, or
 * `but` is refused; the rest is the judgement `skill://trim` asks a reviewer to make. */
export const NAME_SHAPES: readonly Shape[] = [
  {
    shape: 'article or scaffold prefix',
    message: 'carries an article or scaffold prefix',
    matches: (name) => /^(a|an|the|test|it|should|when)_/.test(name),
  },
  {
    shape: 'case position',
    message: "names a case's position instead of the case",
    matches: (name) => /^[a-z]_(first|second|third|fourth|fifth|last|next|other|another)_/.test(name),
  },
  {
    shape: 'connective chain',
    message: 'welds clauses together with connectives',
    matches: (name) => /(?:_(?:and|or|but)_[a-z0-9_]*){2}/.test(name),
  },
  {
    shape: 'connective prose',
    message: 'spells a sentence where a value belongs',
    matches: (name) => /_(?:and|or)_(?:a|an|the|its|their|they|it|this|that|then)_|_but_/.test(name),
  },
  {
    shape: 'dangling article',
    message: 'ends in an article where a value belongs',
    matches: (name) => /_(a|an|the)$/.test(name),
  },
  {
    shape: 'indefinite article',
    message: 'spells an article where a value belongs',
    matches: (name) => /_(a|an)_/.test(name),
  },
  {
    shape: 'bare verdict',
    message: 'says nothing about the outcome',
    matches: (name) => /_(works|succeeds|is_ok|is_handled|passes|functions)$/.test(name),
  },
  {
    shape: 'hedge',
    message: 'hedges the outcome instead of naming it',
    matches: (name) =>
      /_(correctly|properly|successfully|gracefully|cleanly|smoothly|as_expected|as_intended|fine|good|ok|valid)$/
        .test(
          name,
        ),
  },
  {
    shape: 'generic verb',
    message: 'stands in a generic verb for the behaviour',
    matches: (name) => /(^|_)(handles|behaves|does_the_right_thing)$/.test(name),
  },
  {
    shape: 'single word',
    message: 'names a topic instead of a behaviour',
    matches: (name) => /^[a-z0-9]+$/.test(name),
  },
  {
    shape: 'order marker',
    message: "marks a case's order instead of naming it",
    matches: (name) => /_\d+$|_part_\d+$/.test(name),
  },
  {
    shape: 'placeholder word',
    message: 'names the technique that builds the value',
    matches: (name) => /(^|_)(fixture|probe)(_|$)/.test(name),
  },
  {
    shape: 'ceiling',
    message: `is longer than ${CEILING} characters`,
    matches: (name) => name.length > CEILING,
  },
]

/** Shapes a case title never has. A title is a sentence, so the checks for names that read as
 * sentences do not reach it, and neither does the ceiling: a title states what the whole case
 * proves, and the suites' titles are longer than any name. */
export const TITLE_SHAPES: readonly Shape[] = [
  {
    shape: 'scaffold prefix',
    message: 'opens as a scaffold instead of a behaviour',
    matches: (title) => /^(test|it|should|when)\b/i.test(title),
  },
  {
    shape: 'bare verdict',
    message: 'says nothing about the outcome',
    matches: (title) => /\b(works|succeeds|is ok|is handled|passes|functions)\b\.?$/i.test(title),
  },
  {
    shape: 'hedge',
    message: 'hedges the outcome instead of naming it',
    matches: (title) =>
      /\b(correctly|properly|successfully|gracefully|cleanly|smoothly|as expected|as intended|behaves|does the right thing)\b/i
        .test(
          title,
        ),
  },
  {
    shape: 'placeholder word',
    message: 'names the technique that builds the value',
    matches: (title) => /\b(fixture|probe)\b/i.test(title),
  },
  {
    shape: 'single word',
    message: 'names a topic instead of a behaviour',
    matches: (title) => /^\S+$/.test(title),
  },
]

/** Why one test name is refused, or `undefined` when it names a behaviour. */
export function refusal(name: string, shapes: readonly Shape[]): string | undefined {
  for (const shape of shapes) {
    if (shape.matches(name)) return `${shape.message}: ${name}`
  }
  return undefined
}

/** One test the workspace declares, with the file and line that declare it. */
export interface DeclaredTest {
  readonly path: string
  readonly line: number
  readonly name: string
}

/** A test declaration whose name the audit cannot read, and so cannot judge. */
export interface UnreadableTest {
  readonly path: string
  readonly line: number
  readonly detail: string
}

/** Every name one scan read, and every declaration it could not. */
export interface Scan {
  /** Rust test functions. */
  readonly tests: DeclaredTest[]
  /** The case titles of the suites the workspace runs, with `${…}` interpolations blanked. */
  readonly titles: DeclaredTest[]
  readonly unreadable: UnreadableTest[]
}

/** The line `index` falls on. */
function lineOf(source: string, index: number): number {
  let line = 1
  for (let at = 0; at < index; at += 1) {
    if (source[at] === '\n') line += 1
  }
  return line
}

/** The position just past a `//` comment starting at `index`. */
function skipLineComment(source: string, index: number): number {
  const newline = source.indexOf('\n', index + 2)
  return newline === -1 ? source.length : newline + 1
}

/** The position just past a block comment starting at `index`; Rust block comments nest. */
function skipBlockComment(source: string, index: number): number {
  let depth = 0
  let at = index
  while (at < source.length) {
    if (source.startsWith('/*', at)) {
      depth += 1
      at += 2
    } else if (source.startsWith('*/', at)) {
      depth -= 1
      at += 2
      if (depth === 0) return at
    } else {
      at += 1
    }
  }
  return source.length
}

/** The position just past the quoted literal starting at `quote`. */
function skipQuoted(source: string, quote: number): number {
  const delimiter = source[quote]
  let at = quote + 1
  while (at < source.length) {
    if (source[at] === '\\') {
      at += 2
    } else if (source[at] === delimiter) {
      return at + 1
    } else {
      at += 1
    }
  }
  return source.length
}

/** The position just past the raw string at `index`, or `undefined` when none starts there. */
function rawStringEnd(source: string, index: number): number | undefined {
  const opening = /^(?:b?r)(#*)"/.exec(source.slice(index, index + 32))
  if (!opening) return undefined
  const closing = `"${opening[1]}`
  const end = source.indexOf(closing, index + opening[0].length)
  return end === -1 ? source.length : end + closing.length
}

/** The position just past the character literal or lifetime starting at `index`. */
function skipCharacter(source: string, index: number): number {
  if (source[index + 1] === '\\') {
    const end = source.indexOf("'", index + 2)
    return end === -1 ? source.length : end + 1
  }
  return source[index + 2] === "'" ? index + 3 : index + 1
}

/** One attribute the scan found in code, and what it names. */
interface Attribute {
  readonly index: number
  readonly end: number
  /** Its path before any arguments: `cfg` from `cfg(test)`, `tokio::test` from `tokio::test(flavor)`. */
  readonly path: string
}

/** The attribute at `index`, from `#[` through `]`, or `undefined` when its brackets never close. */
function readAttribute(source: string, index: number): Attribute | undefined {
  let at = index + 2
  let depth = 0
  while (at < source.length) {
    const character = source[at]
    if (character === '"') {
      at = skipQuoted(source, at)
    } else if (character === '[') {
      depth += 1
      at += 1
    } else if (character === ']') {
      if (depth === 0) {
        const body = source.slice(index + 2, at)
        return { index, end: at + 1, path: body.split('(', 1)[0]!.trim() }
      }
      depth -= 1
      at += 1
    } else {
      at += 1
    }
  }
  return undefined
}

/** Every attribute `source` writes outside its comments and literals. */
function attributes(source: string): Attribute[] {
  const found: Attribute[] = []
  let at = 0
  while (at < source.length) {
    const character = source[at]!
    if (character === '/' && source[at + 1] === '/') {
      at = skipLineComment(source, at)
    } else if (character === '/' && source[at + 1] === '*') {
      at = skipBlockComment(source, at)
    } else if (character === '"') {
      at = skipQuoted(source, at)
    } else if (character === 'r' || (character === 'b' && source[at + 1] === 'r')) {
      at = rawStringEnd(source, at) ?? at + 1
    } else if (character === "'") {
      at = skipCharacter(source, at)
    } else if (character === '#' && source[at + 1] === '[') {
      const attribute = readAttribute(source, at)
      if (attribute === undefined) break
      found.push(attribute)
      at = attribute.end
    } else {
      at += 1
    }
  }
  return found
}

/** Whether an attribute declares a test: `test`, a runtime's `tokio::test`, or another harness's
 * `rstest` or `test_case`. The test is declared either way, and its name still has to name a
 * behaviour. */
function declaresTest(attribute: Attribute): boolean {
  return attribute.path.split('::').at(-1)!.endsWith('test')
}

const MODIFIERS = /^(?:pub(?:\s*\([^)]*\))?|async|unsafe|const|extern\s+"[^"]*")\s*/

/** The function the item at `index` opens, past its remaining attributes and comments, or
 * `undefined` when no function follows. */
function declaredFunction(source: string, index: number): { name: string; index: number } | undefined {
  let at = index
  while (at < source.length) {
    const character = source[at]!
    if (character === '/' && source[at + 1] === '/') {
      at = skipLineComment(source, at)
    } else if (character === '/' && source[at + 1] === '*') {
      at = skipBlockComment(source, at)
    } else if (character === '#' && source[at + 1] === '[') {
      const attribute = readAttribute(source, at)
      if (attribute === undefined) return undefined
      at = attribute.end
    } else if (/\s/.test(character)) {
      at += 1
    } else {
      const modifier = MODIFIERS.exec(source.slice(at))
      if (modifier) {
        at += modifier[0].length
        continue
      }
      const declared = /^fn\s+([A-Za-z_][A-Za-z0-9_]*)/.exec(source.slice(at))
      return declared?.[1] === undefined ? undefined : { name: declared[1], index: at }
    }
  }
  return undefined
}

/** Every test function `source` declares. */
export function rustTests(
  source: string,
  path: string,
): { tests: DeclaredTest[]; unreadable: UnreadableTest[] } {
  const tests: DeclaredTest[] = []
  const unreadable: UnreadableTest[] = []
  const read = new Set<number>()
  for (const attribute of attributes(source)) {
    if (!declaresTest(attribute)) continue
    const declared = declaredFunction(source, attribute.end)
    if (declared === undefined) {
      unreadable.push({
        path,
        line: lineOf(source, attribute.index),
        detail: 'declares a test this audit cannot read a function from',
      })
    } else if (!read.has(declared.index)) {
      read.add(declared.index)
      tests.push({ path, line: lineOf(source, declared.index), name: declared.name })
    }
  }
  return { tests, unreadable }
}

/** A case declaration: the title follows the call, so a call with no argument at all — a stored
 * callback the suite invokes — is not one. */
const TITLE_CALL = /^[ \t]*(?:await\s+)?(?:test|check)\s*\(\s*(?=['"`]|[^)\s])/gm

/** The position just past the interpolation whose brace is at `brace`, nested templates, strings,
 * and comments included. */
function skipInterpolation(source: string, brace: number): number {
  let depth = 1
  let at = brace + 1
  while (at < source.length) {
    const character = source[at]!
    if (character === "'" || character === '"') {
      at = skipQuoted(source, at)
      continue
    }
    if (character === '`') {
      at = skipTemplate(source, at)
      continue
    }
    if (character === '/' && source[at + 1] === '/') {
      at = skipLineComment(source, at)
      continue
    }
    if (character === '/' && source[at + 1] === '*') {
      at = skipBlockComment(source, at)
      continue
    }
    if (character === '{') depth += 1
    if (character === '}') {
      depth -= 1
      if (depth === 0) return at + 1
    }
    at += 1
  }
  return source.length
}

/** The position just past the template literal starting at `quote`. */
function skipTemplate(source: string, quote: number): number {
  let at = quote + 1
  while (at < source.length) {
    const character = source[at]
    if (character === '\\') {
      at += 2
    } else if (character === '`') {
      return at + 1
    } else if (character === '$' && source[at + 1] === '{') {
      at = skipInterpolation(source, at + 1)
    } else {
      at += 1
    }
  }
  return source.length
}

/** Whether the `/` at `index` opens a regular expression rather than dividing. */
function startsRegExp(source: string, index: number): boolean {
  const before = source.slice(0, index).trimEnd()
  const last = before.at(-1)
  if (last === undefined) return true
  if ('(,=:[!&|?{};+-*%~^<>'.includes(last)) return true
  return /\b(?:return|typeof|case|in|of|new|delete|void|do|else|yield)$/.test(before)
}

/** The position just past the regular expression starting at `index`. */
function skipRegExp(source: string, index: number): number {
  let inClass = false
  let at = index + 1
  while (at < source.length) {
    const character = source[at]
    if (character === '\\') {
      at += 2
    } else if (character === '[') {
      inClass = true
      at += 1
    } else if (character === ']') {
      inClass = false
      at += 1
    } else if (character === '/' && !inClass) {
      at += 1
      while (/[a-z]/.test(source[at] ?? '')) at += 1
      return at
    } else {
      at += 1
    }
  }
  return source.length
}

/** The spans of `source` that are code: comments, strings, template literals, and regular
 * expressions sit outside them, and an interpolation is code inside a template. */
function codeSpans(source: string): [number, number][] {
  const spans: [number, number][] = []
  let codeStart = 0
  let at = 0
  while (at < source.length) {
    const character = source[at]!
    let end: number | undefined
    if (character === '/' && source[at + 1] === '/') {
      end = skipLineComment(source, at)
    } else if (character === '/' && source[at + 1] === '*') {
      end = skipBlockComment(source, at)
    } else if (character === "'" || character === '"') {
      end = skipQuoted(source, at)
    } else if (character === '`') {
      end = skipTemplate(source, at)
    } else if (character === '/' && startsRegExp(source, at)) {
      end = skipRegExp(source, at)
    }
    if (end === undefined) {
      at += 1
      continue
    }
    spans.push([codeStart, at])
    at = end
    codeStart = at
  }
  spans.push([codeStart, source.length])
  return spans
}

/** The `test(` and `check(` calls `source` writes in code: a suite's own source inside a template is
 * not a case, and neither is a call a comment mentions. */
function titleCalls(source: string): { index: number; open: number }[] {
  const spans = codeSpans(source)
  const calls: { index: number; open: number }[] = []
  let span = 0
  for (const call of source.matchAll(TITLE_CALL)) {
    while (spans[span] !== undefined && spans[span]![1] <= call.index) span += 1
    const code = spans[span]
    if (code === undefined || call.index < code[0]) continue
    calls.push({ index: call.index, open: call.index + call[0].length })
  }
  return calls
}

/** The title the case at `open` passes, with interpolations blanked, or `undefined` when its first
 * argument is not a literal the audit can read. */
function callTitle(source: string, open: number): string | undefined {
  let at = open
  while (at < source.length && /\s/.test(source[at]!)) at += 1
  const quote = source[at]
  if (quote === "'" || quote === '"') return source.slice(at + 1, skipQuoted(source, at) - 1)
  if (quote !== '`') return undefined
  let title = ''
  at += 1
  while (at < source.length) {
    const character = source[at]!
    if (character === '\\') {
      title += source.slice(at, at + 2)
      at += 2
    } else if (character === '`') {
      return title
    } else if (character === '$' && source[at + 1] === '{') {
      title += ' '
      at = skipInterpolation(source, at + 1)
    } else {
      title += character
      at += 1
    }
  }
  return undefined
}

/** Every case title `source` writes. A grouping (`describe`) names a body of cases, not a behaviour,
 * so it is not a case title. */
export function typescriptTitles(
  source: string,
  path: string,
): { titles: DeclaredTest[]; unreadable: UnreadableTest[] } {
  const titles: DeclaredTest[] = []
  const unreadable: UnreadableTest[] = []
  for (const call of titleCalls(source)) {
    const title = callTitle(source, call.open)
    const line = lineOf(source, call.index)
    if (title === undefined) {
      unreadable.push({ path, line, detail: 'names a case without a title this audit can read' })
    } else {
      titles.push({ path, line, name: title.trim() })
    }
  }
  return { titles, unreadable }
}

/** Directories the workspace's own sources never live in. */
const ARTIFACTS: Record<string, true> = {
  node_modules: true,
  target: true,
  dist: true,
  'playwright-report': true,
  'test-results': true,
}

/** Every file under `directory` whose name `keeps`, outside the artifact directories. */
function files(directory: string, keeps: (name: string) => boolean): string[] {
  const found: string[] = []
  const pending = [directory]
  while (pending.length > 0) {
    const current = pending.pop()!
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (ARTIFACTS[entry.name] === undefined) pending.push(join(current, entry.name))
      } else if (entry.isFile() && keeps(entry.name)) {
        found.push(join(current, entry.name))
      }
    }
  }
  return found.sort()
}

function declaredTests(): Scan {
  const repository = REPOSITORY_ROOT
  const scan: Scan = { tests: [], titles: [], unreadable: [] }
  const rust = [
    ...files(join(repository, 'crates'), (name) => name.endsWith('.rs')),
    ...files(join(repository, 'src'), (name) => name.endsWith('.rs')),
  ]
  const suites = [
    ...files(join(repository, 'e2e', 'tests'), (name) => name.endsWith('.spec.ts')),
    ...files(join(repository, 'extensions', 'vscode', 'src', 'test'), (name) => name.endsWith('.ts')),
    ...files(join(repository, 'scripts'), (name) => name.endsWith('.test.ts')),
  ]
  for (const file of rust) {
    const path = relative(repository, file)
    const read = rustTests(readFileSync(file, 'utf8'), path)
    scan.tests.push(...read.tests)
    scan.unreadable.push(...read.unreadable)
  }
  for (const file of suites) {
    const path = relative(repository, file)
    const read = typescriptTitles(readFileSync(file, 'utf8'), path)
    scan.titles.push(...read.titles)
    scan.unreadable.push(...read.unreadable)
  }
  return scan
}

if (import.meta.main) {
  const { tests, titles, unreadable } = declaredTests()
  const failures = unreadable.map((test) => `${test.path}:${test.line}: ${test.detail}`)
  for (const test of tests) {
    const refused = refusal(test.name, NAME_SHAPES)
    if (refused) failures.push(`${test.path}:${test.line}: ${refused}`)
  }
  for (const title of titles) {
    const refused = refusal(title.name, TITLE_SHAPES)
    if (refused) failures.push(`${title.path}:${title.line}: ${refused}`)
  }
  if (failures.length > 0) {
    for (const failure of failures) console.error(failure)
    console.error(`\n${failures.length} test name(s) need renaming; see .agents/skills/trim/SKILL.md`)
    process.exit(1)
  }
  console.log(`${tests.length} test names and ${titles.length} case titles name a behaviour`)
}
