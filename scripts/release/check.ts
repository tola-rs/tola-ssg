import { setImmediate } from 'node:timers/promises'
import { parseArgs } from 'node:util'
import { CommandCancelled, withCancellation } from '../cancellation.ts'
import { REPOSITORY_ROOT } from '../paths.ts'
import { errorMessage } from '../process.ts'
import { checkAll, checkPackages, checkSource } from './release-check.ts'
import { parseReleaseMode, type ReleaseMode } from './release-mode.ts'
import { tagVersion } from './semver.ts'

const HELP = `Verify release source, registry availability, and distributable packages; never upload.

Usage: deno run -A scripts/release/check.ts source TAG [--commit COMMIT] [--mode MODE]
       deno run -A scripts/release/check.ts packages TAG [--commit COMMIT] [--mode MODE]
       deno run -A scripts/release/check.ts all TAG

Modes: create, update-preserve-notes, update-regenerate-notes (default: create).
Updates check the selected source and packages without requiring an unused crates.io version.
`

interface CheckArguments {
  positionals: string[]
  values: { commit?: string; mode?: string; help?: boolean }
}

export async function main(args: string[] = process.argv.slice(2)): Promise<void> {
  let parsed: CheckArguments
  let mode: ReleaseMode
  try {
    parsed = parseArgs({
      args,
      strict: true,
      allowPositionals: true,
      options: {
        commit: { type: 'string' },
        mode: { type: 'string' },
        help: { type: 'boolean', short: 'h' },
      },
    })
    if (parsed.values.help) {
      process.stdout.write(HELP)
      return
    }
    const [command] = parsed.positionals
    if (!command || !['source', 'packages', 'all'].includes(command) || parsed.positionals.length !== 2) {
      throw new Error('expected source, packages, or all and a release tag')
    }
    if (command === 'all' && (parsed.values.commit !== undefined || parsed.values.mode !== undefined)) {
      throw new Error('--commit and --mode are only accepted by source or packages')
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
        const [command, tag] = parsed.positionals
        if (tag === undefined) throw new Error('missing release tag')
        tagVersion(tag)
        if (command === 'source') await checkSource(REPOSITORY_ROOT, tag, parsed.values.commit, signal, mode)
        else if (command === 'packages') {
          const source = await checkSource(REPOSITORY_ROOT, tag, parsed.values.commit, signal, mode)
          await checkPackages(REPOSITORY_ROOT, tag, source, signal, mode)
        } else await checkAll(REPOSITORY_ROOT, tag, signal)
      } finally {
        await setImmediate()
      }
    })
  } catch (error) {
    const cancelled = error instanceof CommandCancelled
    process.stderr.write(
      cancelled
        ? `release check cancelled; nothing was published\n${error.message}\n`
        : `error: ${errorMessage(error)}\n`,
    )
    process.exitCode = cancelled ? error.exitCode : 1
  }
}

if (import.meta.main) await main()
