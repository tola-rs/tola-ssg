import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'

export type HookPhase = 'before-build' | 'generate-outputs' | 'after-publish'

export type HookOutput = string | { file: string }

export type HookOptions = {
  name: string
  outputs?: readonly HookOutput[]
  rerunOn?: readonly string[]
  /** Development-session participation; omit to leave the phase's configured default in place. */
  dev?: 'run' | 'skip'
}

/** The flags every hook script is spawned with: it reads and writes site files, reaches the gate
 *  over HTTP, reads the run's roots from its environment, and may spawn the CLI under test. */
const HOOK_PERMISSIONS = [
  'run',
  '--allow-read',
  '--allow-write',
  '--allow-net',
  '--allow-env',
  '--allow-run',
] as const

/** The command runs `script` under Deno, so hooks need no shebang. */
export function hookToml(phase: HookPhase, script: string, options: HookOptions): string {
  let table = `[[build.hooks.${phase}]]\n` +
    `name = ${JSON.stringify(options.name)}\n` +
    `command = ${JSON.stringify([process.execPath, ...HOOK_PERMISSIONS, script])}\n`
  if (options.dev !== undefined) table += `dev = ${JSON.stringify(options.dev)}\n`
  if (options.rerunOn !== undefined) {
    table += `rerun-on = [${options.rerunOn.map((value) => JSON.stringify(value)).join(', ')}]\n`
  }
  if (options.outputs !== undefined) {
    // `before-build` declares what it writes as `generates`; `generate-outputs` declares the
    // output paths it joins to the candidate as `outputs`. The option means both.
    const key = phase === 'before-build' ? 'generates' : 'outputs'
    const outputs = options.outputs.map((output) =>
      typeof output === 'string' ? JSON.stringify(output) : `{ file = ${JSON.stringify(output.file)} }`
    )
    table += `${key} = [${outputs.join(', ')}]\n`
  }
  return table
}

/** Replaces the whole configuration, so hooks from an earlier call do not survive. */
export async function writeHookConfiguration(
  root: string,
  hooks: readonly string[],
  configuration = '',
): Promise<void> {
  await writeFile(join(root, 'tola.toml'), configuration + hooks.join(''))
}

/** The path of a hook run journal beside the site root, where discovery never reads it. */
export function hookJournal(root: string, name = 'hook-invocations.txt'): string {
  return join(dirname(root), name)
}

/** The journal at `path`, empty while no hook run has written it. */
export async function readHookJournal(path: string): Promise<string> {
  try {
    return await readFile(path, 'utf8')
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return ''
    throw error
  }
}

/**
 * Writes the pre-existing `content/extra.txt` a hook repeats a write to: the name exists before the
 * first build and discovery never reads it, so the write cannot replace the candidate however the
 * platform reports it.
 */
export async function writeUnreadContentFile(root: string): Promise<void> {
  await writeFile(join(root, 'content/extra.txt'), 'Pre-existing unread file.\n')
}
