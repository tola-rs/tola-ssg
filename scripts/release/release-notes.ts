import { resolve } from 'node:path'
import { caseFold } from 'unicode-case-folding'
import { runProcess } from '../process.ts'
import { checkTag } from './checkout.ts'
import { ReleaseError } from './release-error.ts'
import { compareVersions, type SemVer, tagVersion, versionKey as semverKey } from './semver.ts'

export type ReleaseCategory = 'Features' | 'Bug Fixes' | 'Refactor' | 'Performance' | 'Other Changes'
export type ReleaseSubjects = Record<ReleaseCategory, string[]>
export interface CategorizedSubject {
  readonly category: ReleaseCategory
  readonly body: string
}

const CATEGORIES: Readonly<Record<string, ReleaseCategory | undefined>> = {
  feat: 'Features',
  fix: 'Bug Fixes',
  refactor: 'Refactor',
  perf: 'Performance',
}
const SKIPPED_KINDS: Readonly<Record<string, true | undefined>> = {
  release: true,
  ci: true,
  chore: true,
  docs: true,
  build: true,
  style: true,
  test: true,
  fmt: true,
}
const COMMIT = /^([A-Za-z]+)(?:\([^)]*\))?!?: *(.+)$/
// deno-lint-ignore no-control-regex -- the information separators are what this pattern matches
const LINE_BREAK = /\r\n|[\n\r\v\f\u001c-\u001e\u0085\u2028\u2029]/

async function gitOutput(root: string, args: readonly string[], signal?: AbortSignal): Promise<string> {
  const result = await runProcess('git', args, { cwd: root, signal, strictUtf8: true })
  signal?.throwIfAborted()
  if (result.exitCode !== 0 || result.timedOut) {
    throw new ReleaseError(`git ${args.join(' ')} failed: ${result.stderr.trim()}`)
  }
  const text = result.stdout.replaceAll('\r\n', '\n').replaceAll('\r', '\n')
  // deno-lint-ignore no-control-regex -- the information separators are what this pattern trims
  return text.replace(/^[\p{White_Space}\u001c-\u001f]+|[\p{White_Space}\u001c-\u001f]+$/gu, '')
}

function versionKey(tag: string): SemVer | null {
  try {
    return semverKey(tagVersion(tag))
  } catch (error) {
    if (error instanceof ReleaseError) return null
    throw error
  }
}

export async function findPreviousTag(
  root: string,
  tag: string,
  signal?: AbortSignal,
): Promise<string | null> {
  const current = versionKey(tag)
  if (current === null) throw new ReleaseError(`release tag must look like v0.x.y or v0.x.y-pre.1: ${tag}`)
  let previousTag: string | null = null
  let previousVersion: SemVer | null = null
  for (const candidate of (await gitOutput(root, ['tag', '--list', 'v[0-9]*'], signal)).split(LINE_BREAK)) {
    const version = versionKey(candidate)
    if (version === null || compareVersions(version, current) >= 0) continue
    if (previousVersion === null || compareVersions(version, previousVersion) >= 0) {
      previousTag = candidate
      previousVersion = version
    }
  }
  return previousTag
}

export async function commitSubjects(
  root: string,
  previousTag: string,
  tag: string,
  signal?: AbortSignal,
): Promise<string[]> {
  // Rebased release lines exclude patch-equivalent changes on the left side.
  const output = await gitOutput(
    root,
    ['log', '--cherry-pick', '--right-only', '--pretty=format:%s', `${previousTag}...${tag}`],
    signal,
  )
  return output ? output.split(LINE_BREAK) : []
}

export function normalizedSubject(subject: string): CategorizedSubject | null {
  // deno-lint-ignore no-control-regex -- the information separators are what this pattern folds
  subject = subject.replace(/[\p{White_Space}\u001c-\u001f]+/gu, ' ').replace(/^ | $/g, '')
  if (!subject) return null
  const match = COMMIT.exec(subject)
  if (match?.[1] !== undefined && match[2] !== undefined) {
    const kind = match[1].toLowerCase()
    const category = Object.hasOwn(CATEGORIES, kind) ? CATEGORIES[kind] : undefined
    if (category) return { category, body: match[2] }
    if (Object.hasOwn(SKIPPED_KINDS, kind)) return null
  }
  const lowered = subject.toLowerCase()
  if (lowered.startsWith('release ') || versionKey(subject) !== null) return null
  if (lowered.startsWith('update readme') || lowered.startsWith('docs ')) return null
  return { category: 'Other Changes', body: subject }
}

export function groupedSubjects(subjects: readonly string[]): ReleaseSubjects {
  const groups: ReleaseSubjects = {
    Features: [],
    'Bug Fixes': [],
    Refactor: [],
    Performance: [],
    'Other Changes': [],
  }
  const seen = new Set<string>()
  for (const subject of subjects) {
    const normalized = normalizedSubject(subject)
    if (!normalized) continue
    const key = `${normalized.category}\0${caseFold(normalized.body)}`
    if (seen.has(key)) continue
    seen.add(key)
    groups[normalized.category].push(normalized.body)
  }
  return groups
}

export async function renderNotes(
  root: string,
  tag: string,
  repository: string,
  commit?: string,
  signal?: AbortSignal,
): Promise<string> {
  signal?.throwIfAborted()
  tagVersion(tag)
  const selected = commit !== undefined
    ? checkTag(resolve(root), tag, commit)
    : await gitOutput(root, ['rev-parse', '--verify', `refs/tags/${tag}^{commit}`], signal)
  const previousTag = await findPreviousTag(root, tag, signal)
  if (previousTag === null) {
    return `${await gitOutput(root, ['log', '-1', '--format=%B', selected], signal)}\n`
  }
  const groups = groupedSubjects(await commitSubjects(root, previousTag, selected, signal))
  const lines = ["## What's Changed", '']
  for (const [title, subjects] of Object.entries(groups)) {
    if (!subjects.length) continue
    lines.push(`### ${title}`, ...subjects.map((subject) => `- ${subject}`), '')
  }
  if (!Object.values(groups).some((subjects) => subjects.length)) {
    lines.push('No user-facing changes in this release.', '')
  }
  lines.push(`**Full Changelog**: https://github.com/${repository}/compare/${previousTag}...${tag}`, '')
  return lines.join('\n')
}
