import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { setImmediate } from 'node:timers/promises'
import { runProcess } from '../process.ts'
import type { WorkspacePackage } from './cargo-metadata.ts'
import { checkTag, requireClean } from './checkout.ts'
import { buildAll } from './packaging.ts'
import { checkAvailable } from './registry.ts'
import { ReleaseError } from './release-error.ts'
import { checkVersion } from './workspace.ts'

export interface CheckedReleaseSource {
  readonly version: string
  readonly packages: readonly WorkspacePackage[]
  readonly commit: string
}

export async function checkSource(
  root: string,
  tag: string,
  commit?: string,
  signal?: AbortSignal,
): Promise<CheckedReleaseSource> {
  signal?.throwIfAborted()
  requireClean(root)
  const selected = checkTag(root, tag, commit)
  const { version, packages } = await checkVersion(root, signal)
  await setImmediate()
  signal?.throwIfAborted()
  requireClean(root)
  checkTag(root, tag, selected)
  console.log(`Release source ${tag}: checked commit ${selected} (${packages.length} workspace packages).`)
  return { version, packages, commit: selected }
}

async function runVisible(
  root: string,
  command: readonly [string, ...string[]],
  signal?: AbortSignal,
  env?: NodeJS.ProcessEnv,
): Promise<void> {
  const [executable, ...args] = command
  const result = await runProcess(executable, args, {
    cwd: root,
    signal,
    stdout: 'inherit',
    stderr: 'inherit',
    ...(env === undefined ? {} : { env }),
  })
  if (result.exitCode !== 0 || result.timedOut) {
    throw new ReleaseError(`${command.join(' ')} failed (${result.exitCode})`)
  }
}

export async function checkPackages(
  root: string,
  tag: string,
  source: CheckedReleaseSource,
  signal?: AbortSignal,
): Promise<void> {
  await checkAvailable(source.packages, source.version, undefined, signal)
  await mkdir(join(root, 'target'), { recursive: true })
  const directory = await mkdtemp(join(root, 'target', 'package-check-'))
  try {
    // The temporary registry is keyed by build path, including its extracted sources.
    await runVisible(
      root,
      ['cargo', 'package', '--workspace', '--locked', '--all-features', '--registry', 'crates-io'],
      signal,
      { ...process.env, CARGO_TARGET_DIR: directory, CARGO_BUILD_BUILD_DIR: directory },
    )
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
  await setImmediate()
  signal?.throwIfAborted()
  requireClean(root)
  checkTag(root, tag, source.commit)
  console.log(
    `Workspace packages for ${tag} passed Cargo packaging and build verification; nothing was uploaded.`,
  )
}

export async function checkAll(root: string, tag: string, signal?: AbortSignal): Promise<void> {
  const source = await checkSource(root, tag, undefined, signal)
  await runVisible(root, ['just', 'ci'], signal)
  await checkPackages(root, tag, source, signal)
  // Only this host's target can be built here; the release workflow builds the rest.
  await buildAll(source.version, { root, output: join(root, 'release') }, signal)
  await setImmediate()
  signal?.throwIfAborted()
  requireClean(root)
  checkTag(root, tag, source.commit)
  console.log(
    `Local release checks passed for ${tag}. Cross-platform CI must also pass on commit ${source.commit} before publication.`,
  )
}
