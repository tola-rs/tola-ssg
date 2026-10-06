import { spawnSync } from 'node:child_process'
import { runProcess } from '../process.ts'
import { ReleaseError } from './release-error.ts'

/** Release checks stop on command errors; captured output remains diagnostic evidence. */
export function run(root: string, command: readonly [string, ...string[]]): string {
  const [executable, ...args] = command
  const completed = spawnSync(executable, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: 'pipe',
    maxBuffer: 64 * 1024 * 1024,
  })
  if (completed.error) {
    throw new ReleaseError(`${command.join(' ')} failed: ${completed.error.message}`, {
      cause: completed.error,
    })
  }
  if (completed.status !== 0) {
    const detail = (completed.stderr || completed.stdout || '').trim()
    throw new ReleaseError(
      `${command.join(' ')} failed (${completed.signal ?? completed.status})${detail ? `:\n${detail}` : ''}`,
    )
  }
  return completed.stdout.trim()
}

export async function runChecked(
  root: string,
  command: readonly [string, ...string[]],
  signal?: AbortSignal,
): Promise<string> {
  signal?.throwIfAborted()
  const [executable, ...args] = command
  const completed = await runProcess(executable, args, { cwd: root, signal, strictUtf8: true })
  signal?.throwIfAborted()
  if (completed.exitCode !== 0 || completed.timedOut) {
    const detail = (completed.stderr || completed.stdout).trim()
    throw new ReleaseError(
      `${command.join(' ')} failed (${completed.exitCode})${detail ? `:\n${detail}` : ''}`,
    )
  }
  return completed.stdout.trim()
}
