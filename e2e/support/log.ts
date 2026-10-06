import { readdir, readFile } from 'node:fs/promises'
import { join } from 'node:path'

export type LogRecord = {
  target: string
  span?: {
    round?: number
    revision?: string
  }
  fields: {
    kind: string
    command?: string
    success?: boolean
    code?: string
    hook?: string
    stream?: string
    output?: string
    round?: number
    revision?: string
  }
}

export async function readLogRecords(path: string): Promise<LogRecord[]> {
  const source = await readFile(path, 'utf8')
  return source.trimEnd().split('\n').map((line) => JSON.parse(line) as LogRecord)
}

/**
 * The records of the newest session log under `root`. A log read while the command is still
 * writing it, and a session that has not recorded anything yet, both read as no records.
 */
export async function readSessionRecords(root: string): Promise<LogRecord[]> {
  try {
    const directory = join(root, '.tola', 'logs')
    const names = (await readdir(directory)).filter((name) => name.endsWith('.jsonl')).sort()
    const newest = names.at(-1)
    if (newest === undefined) return []
    return await readLogRecords(join(directory, newest))
  } catch {
    return []
  }
}

/** The URL the newest session log's serving line names, or `undefined` until it records one. */
export async function servingUrl(root: string): Promise<string | undefined> {
  return (await readSessionRecords(root))
    .map((record) => (record.fields as typeof record.fields & { message?: string }).message)
    .find((message) => message?.startsWith('Serving '))
    ?.slice('Serving '.length)
}

/** The rounds a session log reports a diagnostic with `code` in. */
export function diagnosticRounds(records: LogRecord[], code: string): number[] {
  return records
    .filter((record) => record.fields.kind === 'diagnostic' && record.fields.code === code)
    .map((record) => record.fields.round ?? 0)
}
