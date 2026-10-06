import { resolve } from 'node:path'
import { CommandCancelled, withCancellation } from '../cancellation.ts'
import { REPOSITORY_ROOT } from '../paths.ts'
import { errorMessage } from '../process.ts'
import { buildAll } from './packaging.ts'
import { workspaceVersion } from './workspace.ts'

export async function main(args: readonly string[] = process.argv.slice(2)): Promise<number> {
  if (args[0] === '--help') {
    console.log(
      'Usage: deno run --allow-read --allow-write --allow-run --allow-env scripts/release/build.ts\nBuild and package the release target this host produces into ./release without publishing.\nEvery other target is built by the release workflow on its own runner.',
    )
    return 0
  }
  if (args.length > 0) {
    console.error(`unknown option: ${args[0]}`)
    return 1
  }
  try {
    await withCancellation((signal) =>
      buildAll(workspaceVersion(REPOSITORY_ROOT), {
        root: REPOSITORY_ROOT,
        output: resolve(REPOSITORY_ROOT, 'release'),
      }, signal)
    )
    return 0
  } catch (error) {
    console.error(errorMessage(error))
    return error instanceof CommandCancelled ? error.exitCode : 1
  }
}

if (import.meta.main) process.exitCode = await main()
