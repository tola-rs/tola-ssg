import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { expect, test as base } from '@playwright/test'
import { findBinary } from './binary.ts'

export const COMMAND_TIMEOUT_MS = { short: 10_000, standard: 15_000, build: 30_000 } as const

export const test = base.extend<{ binary: string; directory: string }>({
  binary: async ({}, use) => {
    await use(await findBinary())
  },
  directory: async ({}, use) => {
    const directory = await mkdtemp(join(tmpdir(), 'tola-e2e-'))
    try {
      await use(directory)
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  },
})

/** A command that has exited, with the output the harness captured from it. */
export type FinishedCommand = {
  code: number | null
  signal: NodeJS.Signals | null
  stdout: string
  stderr: string
}

export async function runCommand(
  binary: string,
  args: string[],
  cwd: string,
  timeoutMs: number,
): Promise<FinishedCommand> {
  const command = new RunningProcess(binary, args, cwd)
  const output = captureOutput(command.child)
  command.child.stdin.end()
  try {
    const exit = await command.waitForClose(timeoutMs)
    return { ...exit, stdout: output.stdout(), stderr: output.stderr() }
  } finally {
    await command.terminate()
  }
}

export type CommandRunner = (
  args: string[],
  cwd: string,
  timeoutMs?: number,
) => Promise<FinishedCommand>

/** Runs Tola with `--color never` and the harness's default deadline. */
export function commandRunner(binary: string): CommandRunner {
  return (args: string[], cwd: string, timeoutMs: number = COMMAND_TIMEOUT_MS.standard) =>
    runCommand(binary, [...args, '--color', 'never'], cwd, timeoutMs)
}

export function expectExited(
  result: { code: number | null; signal: NodeJS.Signals | null; stderr: string },
  code = 0,
): void {
  expect(result.signal).toBeNull()
  expect(result.code, result.stderr).toBe(code)
}

export type RunningCommand = {
  command: RunningProcess
  stdout: () => string
  stderr: () => string
}

/** Starts a Tola command whose output the caller reads while it runs. */
export function startCommand(
  binary: string,
  args: string[],
  cwd: string,
  environment: NodeJS.ProcessEnv = {},
): RunningCommand {
  const command = new RunningProcess(binary, args, cwd, environment)
  const output = captureOutput(command.child)
  command.child.stdin.end()
  return { command, ...output }
}

type ExitStatus = {
  code: number | null
  signal: NodeJS.Signals | null
}

function processEnvironment(overrides: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  const environment = { ...process.env }
  for (const name of Object.keys(environment)) {
    const normalized = name.toUpperCase()
    if (
      normalized.startsWith('TOLA_') || normalized.startsWith('TYPST_') ||
      normalized === 'RUST_LOG' || normalized === 'NO_COLOR' ||
      normalized === 'CLICOLOR' || normalized === 'CLICOLOR_FORCE'
    ) {
      delete environment[name]
    }
  }
  return { ...environment, ...overrides }
}

export function captureOutput(child: ChildProcessWithoutNullStreams): {
  stdout: () => string
  stderr: () => string
} {
  let stdout = ''
  let stderr = ''
  child.stdout.setEncoding('utf8')
  child.stderr.setEncoding('utf8')
  const appendStdout = (chunk: string) => {
    stdout += chunk
  }
  const appendStderr = (chunk: string) => {
    stderr += chunk
  }
  child.stdout.on('data', appendStdout)
  child.stderr.on('data', appendStderr)
  child.once('close', () => {
    child.stdout.removeListener('data', appendStdout)
    child.stderr.removeListener('data', appendStderr)
  })
  return { stdout: () => stdout, stderr: () => stderr }
}

/** Owns a command's completion, including closure of its inherited output pipes. */
export class RunningProcess {
  readonly child: ChildProcessWithoutNullStreams
  private failure: Error | undefined
  private exitStatus: ExitStatus | undefined
  private readonly closed: Promise<ExitStatus>

  constructor(command: string, args: string[], cwd: string, environment: NodeJS.ProcessEnv = {}) {
    this.child = spawn(command, args, {
      cwd,
      env: processEnvironment(environment),
      detached: process.platform !== 'win32',
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    const recordFailure = (error: Error) => {
      this.failure ??= error
    }
    this.child.on('error', recordFailure)
    this.child.stdin.on('error', recordFailure)
    this.child.stdout.on('error', recordFailure)
    this.child.stderr.on('error', recordFailure)
    this.closed = new Promise((resolveExit) => {
      this.child.once('close', (code, signal) => {
        this.child.removeListener('error', recordFailure)
        this.child.stdin.removeListener('error', recordFailure)
        this.child.stdout.removeListener('error', recordFailure)
        this.child.stderr.removeListener('error', recordFailure)
        this.exitStatus = { code, signal }
        resolveExit(this.exitStatus)
      })
    })
  }

  get exit(): ExitStatus | undefined {
    return this.exitStatus
  }

  get error(): Error | undefined {
    return this.failure
  }

  async waitForClose(timeoutMs: number): Promise<ExitStatus> {
    const exit = await this.closeWithin(timeoutMs)
    if (this.failure) throw this.failure
    return exit
  }

  private async closeWithin(timeoutMs: number): Promise<ExitStatus> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([
        this.closed,
        new Promise<never>((_, reject) => {
          timeout = setTimeout(() => {
            reject(
              new Error(`Command did not close within ${timeoutMs} ms: ${this.child.spawnargs.join(' ')}`),
            )
          }, timeoutMs)
        }),
      ])
    } finally {
      clearTimeout(timeout)
    }
  }

  /**
   * Delivers an interrupt the way a terminal would: on Unix to the command's whole process group.
   * Every command is spawned as its own group leader: signalling the group leader alone leaves the
   * command dying from the default disposition instead of its own shutdown.
   */
  interrupt(): void {
    const pid = this.child.pid
    if (pid === undefined) return
    if (process.platform === 'win32') {
      this.child.kill('SIGINT')
      return
    }
    try {
      process.kill(-pid, 'SIGINT')
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
    }
  }

  async terminate(): Promise<void> {
    if (this.exitStatus !== undefined) return
    const pid = this.child.pid
    if (pid !== undefined) {
      if (process.platform === 'win32') {
        if (this.child.exitCode === null && this.child.signalCode === null) {
          const termination = new RunningProcess('taskkill', ['/PID', String(pid), '/T', '/F'], process.cwd())
          const output = captureOutput(termination.child)
          termination.child.stdin.end()
          try {
            const exit = await termination.waitForClose(5_000)
            if (exit.code !== 0 && this.child.exitCode === null && this.child.signalCode === null) {
              throw new Error(`Could not terminate command ${pid}: ${output.stderr()}`)
            }
          } finally {
            if (
              termination.child.pid !== undefined &&
              termination.child.exitCode === null && termination.child.signalCode === null
            ) {
              termination.child.kill('SIGKILL')
              await termination.closeWithin(5_000)
            }
          }
        }
      } else {
        try {
          process.kill(-pid, 'SIGKILL')
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
        }
      }
    }
    await this.closeWithin(5_000)
  }
}
