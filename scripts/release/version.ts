import { setImmediate } from 'node:timers/promises'
import { parseArgs } from 'node:util'
import { CommandCancelled, withCancellation } from '../cancellation.ts'
import { REPOSITORY_ROOT } from '../paths.ts'
import { errorMessage } from '../process.ts'
import { checkTag } from './checkout.ts'
import { ReleaseError } from './release-error.ts'
import { updateVersion } from './version-update.ts'
import { checkVersion } from './workspace.ts'

const HELP = `Check and update the synchronized Rust workspace without publishing.

Usage: deno run --allow-read --allow-write --allow-run=cargo,git --allow-env scripts/release/version.ts preview VERSION
       deno run --allow-read --allow-write --allow-run=cargo,git --allow-env scripts/release/version.ts set VERSION
       deno run --allow-read --allow-write --allow-run=cargo,git --allow-env scripts/release/version.ts check [--tag TAG]
`

interface VersionArguments {
  positionals: string[]
  values: { tag?: string; commit?: string; help?: boolean }
}

export async function main(args: string[] = process.argv.slice(2)): Promise<void> {
  let parsed: VersionArguments
  try {
    parsed = parseArgs({
      args,
      strict: true,
      allowPositionals: true,
      options: { tag: { type: 'string' }, commit: { type: 'string' }, help: { type: 'boolean', short: 'h' } },
    })
    if (parsed.values.help) {
      process.stdout.write(HELP)
      return
    }
    const [command] = parsed.positionals
    if (!command || !['preview', 'set', 'check'].includes(command)) {
      throw new Error('expected preview, set, or check')
    }
    if (parsed.positionals.length !== (command === 'preview' || command === 'set' ? 2 : 1)) {
      throw new Error('unexpected or missing positional argument')
    }
    if (command !== 'check' && (parsed.values.tag !== undefined || parsed.values.commit !== undefined)) {
      throw new Error('--tag and --commit are only accepted by check')
    }
  } catch (error) {
    process.stderr.write(`${HELP}\nerror: ${errorMessage(error)}\n`)
    process.exitCode = 2
    return
  }
  try {
    await withCancellation(async (signal) => {
      try {
        const [command, version] = parsed.positionals
        if (command === 'check') {
          if (parsed.values.commit !== undefined && parsed.values.tag === undefined) {
            throw new ReleaseError('--commit requires --tag')
          }
          if (parsed.values.tag !== undefined) {
            checkTag(REPOSITORY_ROOT, parsed.values.tag, parsed.values.commit)
          }
          const checked = await checkVersion(REPOSITORY_ROOT, signal)
          console.log(
            `Workspace version ${checked.version}: ${checked.packages.length} packages, internal dependencies, Cargo.lock, and Nix agree.`,
          )
        } else {
          if (version === undefined) throw new ReleaseError('missing version')
          await updateVersion(REPOSITORY_ROOT, version, command === 'set', signal)
        }
      } finally {
        await setImmediate()
      }
    })
  } catch (error) {
    const cancelled = error instanceof CommandCancelled
    process.stderr.write(
      cancelled ? `version operation cancelled\n${error.message}\n` : `error: ${errorMessage(error)}\n`,
    )
    process.exitCode = cancelled ? error.exitCode : 1
  }
}

if (import.meta.main) await main()
