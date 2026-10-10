import { type ChildProcess, spawn } from 'node:child_process'
import * as vscode from 'vscode'
import { LanguageClient, type StreamInfo } from 'vscode-languageclient/node'
import type { ServedWorkspace } from './selection.ts'

export class LanguageService {
  private child: ChildProcess | undefined
  private phase: 'starting' | 'running' | 'stopping' = 'starting'
  private startup: vscode.Disposable | undefined
  private killTimer: NodeJS.Timeout | undefined
  private exited: Promise<void> = Promise.resolve()

  constructor(
    private readonly selected: ServedWorkspace,
    private readonly output: vscode.OutputChannel,
    private readonly failed: () => void,
  ) {}

  start(lifetime: vscode.CancellationToken): StreamInfo {
    if (lifetime.isCancellationRequested) throw new vscode.CancellationError()
    const child = spawn(this.selected.command, this.selected.args, {
      cwd: this.selected.folder.uri.fsPath,
      shell: false,
    })
    this.child = child
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
    return { reader: child.stdout!, writer: child.stdin! }
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
    child.kill('SIGINT')
    this.killTimer = setTimeout(() => child.kill('SIGKILL'), 5_000)
    this.killTimer.unref()
  }

  async close(): Promise<void> {
    this.stop()
    await this.exited
  }
}

/** Sites owns failure notifications; the client keeps transport details in the output channel. */
export class TolaClient extends LanguageClient {
  override error(message: string, data?: unknown): void {
    super.error(message, data, false)
  }
}
