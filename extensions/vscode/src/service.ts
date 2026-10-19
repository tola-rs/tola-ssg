import { type ChildProcess, spawn } from 'node:child_process'
import { once } from 'node:events'
import { Writable } from 'node:stream'
import * as vscode from 'vscode'
import { type InitializeParams, LanguageClient, State, type StreamInfo } from 'vscode-languageclient/node'
import type { ServedWorkspace } from './selection.ts'

export class LanguageService {
  private child: ChildProcess | undefined
  private input: Writable | undefined
  private phase: 'starting' | 'running' | 'stopping' = 'starting'
  private startup: vscode.Disposable | undefined
  private killTimer: NodeJS.Timeout | undefined
  private exited: Promise<void> = Promise.resolve()

  constructor(
    private readonly selected: ServedWorkspace,
    private readonly output: vscode.OutputChannel,
    private readonly failed: () => void,
  ) {}

  async start(lifetime: vscode.CancellationToken): Promise<StreamInfo> {
    if (lifetime.isCancellationRequested) throw new vscode.CancellationError()
    const child = spawn(this.selected.command, this.selected.args, {
      cwd: this.selected.folder.uri.fsPath,
      shell: false,
    })
    this.child = child
    // Cancellation can precede the client's initialize write. Its queued writes still settle,
    // but a stopped service receives no more bytes through its closed pipe.
    const input = new Writable({
      highWaterMark: 64 * 1024,
      write: (chunk, encoding, completed) => {
        if (this.stopRequested) return completed()
        child.stdin!.write(chunk, encoding, (error) => completed(this.stopRequested ? undefined : error))
      },
    })
    this.input = input
    this.output.appendLine(`Starting language services for ${this.selected.folder.name}.`)
    child.stderr?.on('data', (chunk: Buffer) => this.output.append(chunk.toString()))
    child.once('error', (error: Error) => this.output.appendLine(error.message))
    this.exited = new Promise<void>((resolve) => {
      child.once('close', (code, signal) => {
        this.startup?.dispose()
        clearTimeout(this.killTimer)
        this.output.appendLine(
          `Language service for ${this.selected.folder.name} exited (${
            signal ?? `exit ${code ?? 'unknown'}`
          }).`,
        )
        if (this.phase === 'running') this.failed()
        resolve()
      })
    })
    // The client cannot dispose while initialize is pending; cancelling that wait closes its transport.
    this.startup = lifetime.onCancellationRequested(() => this.stop())
    if (lifetime.isCancellationRequested) this.stop()
    await once(child, 'spawn')
    if (lifetime.isCancellationRequested) throw new vscode.CancellationError()
    return { reader: child.stdout!, writer: input }
  }

  ready(): void {
    if (this.phase === 'starting') this.phase = 'running'
    this.detachStartup()
  }

  detachStartup(): void {
    this.startup?.dispose()
    this.startup = undefined
  }

  get stopRequested(): boolean {
    return this.phase === 'stopping'
  }

  stop(): void {
    if (this.stopRequested) return
    this.phase = 'stopping'
    this.detachStartup()
    const child = this.child
    if (!child || child.exitCode !== null || child.signalCode !== null) return
    child.stdin?.destroy()
    child.kill('SIGINT')
    this.killTimer = setTimeout(() => child.kill('SIGKILL'), 5_000)
    this.killTimer.unref()
  }

  async close(): Promise<void> {
    this.stop()
    await this.exited
    this.input?.destroy()
  }
}

/** Sites owns failure notifications; the client keeps transport details in the output channel. */
export class TolaClient extends LanguageClient {
  private starting: Promise<void> | undefined

  protected override fillInitializeParams(params: InitializeParams): void {
    super.fillInitializeParams(params)
    // The extension already watches the selected source and package roots.
    const watches = params.capabilities.workspace?.didChangeWatchedFiles
    if (watches) watches.dynamicRegistration = false
  }

  override start(): Promise<void> {
    if (this.starting) return this.starting
    // Both calls share one initialization. Version 9 can clear its readiness promise on close
    // before the outer start returns, so both rejections must remain owned by this caller.
    const started = super.start()
    const ready = super.start()
    const starting = Promise.all([started, ready]).then(() => {})
    this.starting = starting
    void starting.then(
      () => {
        this.starting = undefined
      },
      () => {
        this.starting = undefined
      },
    )
    return starting
  }

  override async stop(timeout?: number): Promise<void> {
    await this.starting?.catch(() => {})
    if (this.state !== State.Stopped) await super.stop(timeout)
  }

  override async dispose(timeout?: number): Promise<void> {
    try {
      await super.dispose(timeout)
    } finally {
      // A failed start can allocate diagnostics before it has a connection to clean them up.
      this.diagnostics?.dispose()
    }
  }

  override error(message: string, data?: unknown): void {
    super.error(message, data, false)
  }
}
