import { writeFileSync } from 'node:fs'
import { setImmediate } from 'node:timers/promises'
import { parseArgs } from 'node:util'
import { CommandCancelled, withCancellation } from '../cancellation.ts'
import { errorMessage } from '../process.ts'
import { parseReleaseMode, type ReleaseMode } from './release-mode.ts'
import { renderNotes } from './release-notes.ts'

const HELP =
  `Usage: deno run --allow-read --allow-write --allow-run=git --allow-env scripts/release/notes.ts --tag TAG --repository OWNER/REPO --output FILE [--repo PATH] [--commit COMMIT] [--mode MODE]
Generate notes for the selected release without changing Git refs.
Modes: create, update-preserve-notes, update-regenerate-notes (default: create).
`
interface NotesArguments {
  positionals: string[]
  values: {
    tag?: string
    repository?: string
    output?: string
    repo?: string
    commit?: string
    mode?: string
    help?: boolean
  }
}

export async function main(args: string[] = process.argv.slice(2)): Promise<void> {
  let parsed: NotesArguments
  let mode: ReleaseMode
  try {
    parsed = parseArgs({
      args,
      strict: true,
      options: {
        tag: { type: 'string' },
        repository: { type: 'string' },
        output: { type: 'string' },
        repo: { type: 'string', default: '.' },
        commit: { type: 'string' },
        mode: { type: 'string' },
        help: { type: 'boolean', short: 'h' },
      },
    })
    if (parsed.values.help) {
      process.stdout.write(HELP)
      return
    }
    if (
      parsed.values.tag === undefined ||
      parsed.values.repository === undefined ||
      parsed.values.output === undefined
    ) {
      throw new Error('--tag, --repository, and --output are required')
    }
    mode = parseReleaseMode(parsed.values.mode)
  } catch (error) {
    process.stderr.write(`${HELP}\nerror: ${errorMessage(error)}\n`)
    process.exitCode = 2
    return
  }
  try {
    await withCancellation(async (signal) => {
      try {
        const { tag, repository, output, repo, commit } = parsed.values
        if (tag === undefined || repository === undefined || output === undefined) {
          throw new Error('missing release notes arguments')
        }
        const notes = await renderNotes(repo ?? '.', tag, repository, commit, signal, mode)
        await setImmediate()
        signal.throwIfAborted()
        writeFileSync(output, notes, 'utf8')
      } finally {
        await setImmediate()
      }
    })
  } catch (error) {
    process.stderr.write(`error: ${errorMessage(error)}\n`)
    process.exitCode = error instanceof CommandCancelled ? error.exitCode : 1
  }
}

if (import.meta.main) await main()
