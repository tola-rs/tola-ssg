/**
 * @module
 * Execution of one external command: both output streams captured as they arrive, a deadline and an
 * abort signal that terminate the whole process tree, and the exit status the caller reports. Native
 * release tooling (cargo, git, the archive tools, the built binary) runs through here.
 */

import type { ChildProcess } from 'node:child_process'
import { spawn } from 'node:child_process'
import { constants } from 'node:os'
import type { Readable } from 'node:stream'

export interface ProcessOptions {
  cwd?: string | undefined
  env?: Record<string, string | undefined> | undefined
  timeoutMs?: number | undefined
  signal?: AbortSignal | undefined
  strictUtf8?: boolean | undefined
  stdout?: 'pipe' | 'inherit' | undefined
  stderr?: 'pipe' | 'inherit' | undefined
}

export interface ProcessOutput {
  stdout: string
  stderr: string
  exitCode: number
  timedOut: boolean
}

/** How a child ended: its exit status, or the error that left it unstarted. */
type ChildOutcome = { kind: 'exited'; status: number } | { kind: 'unstarted'; error: Error }

/** Why a running command was asked to stop; both reasons run the same termination. */
type StopReason = { kind: 'timeout' } | { kind: 'aborted'; reason: unknown }

/** One output stream, decoded as it arrives so a slow command never buffers unbounded output. */
class CapturedOutput {
  readonly completed: Promise<void>
  readonly #stream: Readable
  readonly #chunks: string[] = []
  readonly #decoder: TextDecoder

  constructor(stream: Readable, strictUtf8 = false) {
    this.#stream = stream
    this.#decoder = new TextDecoder('utf-8', { fatal: strictUtf8, ignoreBOM: strictUtf8 })
    this.completed = this.#read()
  }

  get text(): string {
    return this.#chunks.join('')
  }

  /** Drop the pipe, so a descendant holding the same descriptor cannot keep the read open. */
  async close(): Promise<void> {
    this.#stream.destroy()
    await this.completed.catch(() => {})
  }

  async #read(): Promise<void> {
    try {
      for await (const chunk of this.#stream as AsyncIterable<Uint8Array>) {
        this.#chunks.push(this.#decoder.decode(chunk, { stream: true }))
      }
    } catch (error) {
      // A command that never started, or one the system killed, has its pipes destroyed instead of
      // closed. The bytes already read are everything it produced, and the exit status reports why.
      if (!(error instanceof Error && 'code' in error && error.code === 'ERR_STREAM_PREMATURE_CLOSE')) {
        throw error
      }
    } finally {
      // Flush a byte sequence the last chunk left incomplete; strict decoding reports it here.
      this.#chunks.push(this.#decoder.decode())
    }
  }
}

/** The request that ends a command's wait: the first timeout or abort ends it, a second escalates. */
class StopRequest {
  readonly reason: Promise<StopReason>
  readonly escalated: Promise<void>
  readonly #signal: AbortSignal | undefined
  readonly #onAbort: () => void
  #received = false

  constructor(signal: AbortSignal | undefined, timeoutMs: number | undefined) {
    const first = Promise.withResolvers<StopReason>()
    const second = Promise.withResolvers<void>()
    this.reason = first.promise
    this.escalated = second.promise
    this.#signal = signal
    const request = (reason: StopReason) => {
      if (this.#received) second.resolve()
      else {
        this.#received = true
        first.resolve(reason)
      }
    }
    this.#onAbort = () => request({ kind: 'aborted', reason: signal?.reason })
    signal?.addEventListener('abort', this.#onAbort, { once: true })
    if (timeoutMs !== undefined) {
      // Unref'd: a deadline must not keep the process alive once its command has finished.
      setTimeout(() => request({ kind: 'timeout' }), timeoutMs).unref()
    }
    if (signal?.aborted) this.#onAbort()
  }

  /** Release the caller's signal once the command has finished stopping. */
  detach(): void {
    this.#signal?.removeEventListener('abort', this.#onAbort)
  }
}

/**
 * How the child ends. The exit event fires once, so every waiter shares one promise: asking twice
 * for an outcome must not wait for an event that has already happened.
 */
function childOutcome(child: ChildProcess): Promise<ChildOutcome> {
  const { promise, resolve } = Promise.withResolvers<ChildOutcome>()
  child.once('error', (error) => resolve({ kind: 'unstarted', error }))
  child.once('exit', (code, signal) => resolve({ kind: 'exited', status: code ?? killedStatus(signal) }))
  return promise
}

/** A child the signal killed has no exit code, so report the status a shell reports. */
function killedStatus(signal: string | null): number {
  const signals: Record<string, number> = constants.signals
  return 128 + (signal === null ? 0 : (signals[signal] ?? 0))
}

/** Whether `completion` settled in time; `forced` cuts a wait short when a stop escalates. */
async function settlesWithin(
  completion: Promise<unknown>,
  milliseconds: number,
  forced?: Promise<void>,
): Promise<boolean> {
  const expiry = Promise.withResolvers<boolean>()
  setTimeout(() => expiry.resolve(false), milliseconds).unref()
  return await Promise.race([
    completion.then(() => true),
    expiry.promise,
    ...(forced === undefined ? [] : [forced.then(() => false)]),
  ])
}

async function terminateTree(child: ChildProcess, signal: 'SIGTERM' | 'SIGKILL'): Promise<void> {
  const pid = child.pid
  if (pid === undefined) return
  if (process.platform !== 'win32') {
    try {
      // The child leads its own group, so one signal reaches every descendant it started.
      process.kill(-pid, signal)
    } catch (error) {
      if (!(error instanceof Error && 'code' in error && error.code === 'ESRCH')) throw error
    }
    return
  }

  // Windows has no process group to signal, and taskkill cannot recover descendants after their
  // root has exited, so the tree is terminated while the root is still running.
  const killer = spawn('taskkill', ['/PID', String(pid), '/T', '/F'], {
    stdio: ['ignore', 'ignore', 'pipe'],
    windowsHide: true,
  })
  const outcome = childOutcome(killer)
  const diagnostic = killer.stderr === null ? undefined : new CapturedOutput(killer.stderr)
  try {
    const completion = Promise.allSettled([outcome, diagnostic?.completed])
    if (!(await settlesWithin(completion, 1_500)) || killer.exitCode !== 0) {
      throw new Error(`unable to verify termination of process tree ${pid}: ${diagnostic?.text.trim()}`)
    }
  } finally {
    try {
      if (killer.exitCode === null) killer.kill('SIGKILL')
    } finally {
      await settlesWithin(Promise.allSettled([diagnostic?.close(), outcome]), 1_000)
      killer.unref()
    }
  }
}

/** Signal the tree, then force it; failures are returned rather than thrown at the deadline. */
async function stopProcess(
  child: ChildProcess,
  outcome: Promise<ChildOutcome>,
  captures: readonly CapturedOutput[],
  stop: StopRequest,
): Promise<string[]> {
  const failures: string[] = []
  const settled = Promise.allSettled([outcome, ...captures.map((capture) => capture.completed)])
  try {
    await terminateTree(child, 'SIGTERM')
  } catch (error) {
    failures.push(errorMessage(error))
  }
  const graceful = await settlesWithin(settled, 1_000, stop.escalated)
  // A descendant can close its pipes and outlive the root after ignoring SIGTERM.
  if (!graceful || process.platform !== 'win32') {
    try {
      await terminateTree(child, 'SIGKILL')
    } catch (error) {
      failures.push(errorMessage(error))
    }
  }
  if (!graceful) {
    if (child.exitCode === null) {
      try {
        child.kill('SIGKILL')
      } catch (error) {
        failures.push(`unable to terminate child ${child.pid}: ${errorMessage(error)}`)
      }
    }
    if (!(await settlesWithin(settled, 1_000))) {
      failures.push(
        `process tree ${child.pid} did not close its output; descendant termination is unverified`,
      )
      await settlesWithin(Promise.allSettled(captures.map((capture) => capture.close())), 1_000)
      child.unref()
    }
  }
  return [...new Set(failures)]
}

/** Capture or inherit output; deadlines include captured pipes, not just the root process. */
export async function runProcess(
  command: string,
  args: readonly string[],
  options: ProcessOptions = {},
): Promise<ProcessOutput> {
  options.signal?.throwIfAborted()
  const child = spawn(command, [...args], {
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    ...(options.env === undefined ? {} : { env: options.env }),
    stdio: ['ignore', options.stdout ?? 'pipe', options.stderr ?? 'pipe'],
    // A leader of its own group is what lets the deadline reach the descendants it started.
    detached: process.platform !== 'win32',
  })

  const outcome = childOutcome(child)
  const stdout = child.stdout === null ? undefined : new CapturedOutput(child.stdout, options.strictUtf8)
  const stderr = child.stderr === null ? undefined : new CapturedOutput(child.stderr, options.strictUtf8)
  const captures = [stdout, stderr].filter((capture): capture is CapturedOutput => capture !== undefined)
  const stop = new StopRequest(options.signal, options.timeoutMs)
  let shutdown: Promise<string[]> | undefined
  try {
    const settled = await Promise.race([
      Promise.all([stdout?.completed, stderr?.completed, outcome]).then(([, , exit]) => exit),
      stop.reason,
    ])
    if (settled.kind === 'exited') {
      options.signal?.throwIfAborted()
      return {
        stdout: stdout?.text ?? '',
        stderr: stderr?.text ?? '',
        exitCode: settled.status,
        timedOut: false,
      }
    }
    if (settled.kind === 'unstarted') {
      await Promise.allSettled(captures.map((capture) => capture.close()))
      return { stdout: '', stderr: errorMessage(settled.error), exitCode: 127, timedOut: false }
    }
    shutdown = stopProcess(child, outcome, captures, stop)
    const failures = await shutdown
    if (settled.kind === 'aborted') throw settled.reason
    options.signal?.throwIfAborted()
    return {
      stdout: stdout?.text ?? '',
      stderr: [(stderr?.text ?? '').trimEnd(), `timed out after ${options.timeoutMs}ms`, ...failures]
        .filter((line) => line.length > 0)
        .join('\n'),
      exitCode: 124,
      timedOut: true,
    }
  } catch (error) {
    const failures = await (shutdown ?? stopProcess(child, outcome, captures, stop))
    if (failures.length > 0) {
      throw new AggregateError(
        [error, ...failures],
        [`command ${command}: ${errorMessage(error)}`, ...failures].join('\n'),
      )
    }
    throw error
  } finally {
    stop.detach()
  }
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
