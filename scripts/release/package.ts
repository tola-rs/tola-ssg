import { parseArgs } from 'node:util'
import { CommandCancelled, withCancellation } from '../cancellation.ts'
import { REPOSITORY_ROOT } from '../paths.ts'
import { errorMessage } from '../process.ts'
import { build, checksums, verifyDirectory, writeLicenseFiles } from './packaging.ts'
import { TARGET_NAMES, type TargetName, targetName } from './targets.ts'
import { workspaceVersion } from './workspace.ts'

type PackageCommand =
  | { readonly command: 'build'; readonly target: TargetName; readonly output: string }
  | { readonly command: 'verify'; readonly directory: string; readonly target?: TargetName }
  | { readonly command: 'checksums'; readonly directory: string; readonly target?: TargetName }
  | { readonly command: 'licenses'; readonly output: string }
  | { readonly command: 'help'; readonly topic?: string }

function parseCommand(args: readonly string[]): PackageCommand {
  const { values, positionals } = parseArgs({
    args: [...args],
    allowPositionals: true,
    options: {
      help: { type: 'boolean', short: 'h' },
      output: { type: 'string' },
      target: { type: 'string' },
    },
  })
  const [command, operand] = positionals
  if (values.help) return { command: 'help', ...(command === undefined ? {} : { topic: command }) }
  if (command === 'licenses') {
    if (positionals.length !== 1 || values.output === undefined || values.target !== undefined) {
      throw new Error('licenses requires --output and does not accept a target')
    }
    return { command, output: values.output }
  }
  if (positionals.length !== 2 || operand === undefined) {
    throw new Error('a command and its target or directory are required')
  }
  switch (command) {
    case 'build':
      if (values.output === undefined || values.target !== undefined) {
        throw new Error('build requires --output and does not accept --target')
      }
      return { command, target: targetName(operand), output: values.output }
    case 'verify':
      if (values.output !== undefined) throw new Error('verify does not accept --output')
      return {
        command,
        directory: operand,
        ...(values.target === undefined ? {} : { target: targetName(values.target) }),
      }
    case 'checksums':
      if (values.output !== undefined) throw new Error('checksums does not accept --output')
      return {
        command,
        directory: operand,
        ...(values.target === undefined ? {} : { target: targetName(values.target) }),
      }
    default:
      throw new Error(`unknown command: ${command}`)
  }
}

function help(topic?: string): string {
  switch (topic) {
    case 'licenses':
      return 'Usage: deno run --allow-read --allow-write --allow-env scripts/release/package.ts licenses --output DIRECTORY\nWrite the release license material into the output directory as one archive.'
    case 'build':
      return 'Usage: deno run --allow-read --allow-write --allow-run --allow-env scripts/release/package.ts build TARGET --output DIRECTORY\nBuild one Cargo target and atomically replace its verified archive.'
    case 'verify':
      return 'Usage: deno run --allow-read --allow-write --allow-run --allow-env scripts/release/package.ts verify DIRECTORY [--target TARGET]\nVerify exactly the release archives and any checksums.'
    case 'checksums':
      return 'Usage: deno run --allow-read --allow-write --allow-run --allow-env scripts/release/package.ts checksums DIRECTORY [--target TARGET]\nVerify the selected archives and atomically write SHA-256 checksums.'
    default:
      return 'Usage: deno run --allow-read --allow-write --allow-run --allow-env scripts/release/package.ts {build,verify,checksums,licenses} ...\nBuild and verify local release archives without publishing.\n\nCommands:\n  build       Build one Cargo target and atomically replace its archive\n  verify      Verify exactly the release archives and any checksums\n  checksums   Verify the selected archives and atomically write SHA-256 checksums\n  licenses    Write the release license material into the output directory as one archive'
  }
}

export async function main(args: readonly string[] = process.argv.slice(2)): Promise<number> {
  let command: PackageCommand
  try {
    command = parseCommand(args)
  } catch (error) {
    console.error(`${help()}\npackage: ${errorMessage(error)}`)
    return 2
  }
  if (command.command === 'help') {
    console.log(help(command.topic))
    return 0
  }
  try {
    return await withCancellation(async (signal) => {
      const version = workspaceVersion(REPOSITORY_ROOT)
      switch (command.command) {
        case 'licenses':
          await writeLicenseFiles({ root: REPOSITORY_ROOT, output: command.output }, version, signal)
          break
        case 'build':
          await build(command.target, version, { root: REPOSITORY_ROOT, output: command.output }, signal)
          break
        case 'verify':
          await verifyDirectory(
            command.directory,
            version,
            command.target === undefined ? {} : { target: command.target },
            signal,
          )
          break
        case 'checksums':
          await checksums(
            command.directory,
            version,
            command.target === undefined ? TARGET_NAMES : [command.target],
            signal,
          )
          break
      }
      return 0
    })
  } catch (error) {
    console.error(`package: ${errorMessage(error)}`)
    return error instanceof CommandCancelled ? error.exitCode : 1
  }
}

if (import.meta.main) process.exitCode = await main()
