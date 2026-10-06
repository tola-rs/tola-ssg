import { type ChildProcess, spawn } from 'node:child_process'
import * as vscode from 'vscode'

import type { Selection } from './selection.ts'

const SERVING_LINE = /^Serving (http:\/\/\S+)/m

export interface PublishedRoute {
  readonly output: string
  readonly route: string
}

export interface Publication {
  readonly revision: string
  readonly routes: PublishedRoute[]
}

export class PreviewServer {
  readonly ready: Promise<URL>
  readonly exited: Promise<void>
  private readonly child: ChildProcess
  private readonly cancellation = new AbortController()
  private readonly lifetime: vscode.Disposable
  private address: URL | undefined
  private stopped = false
  private closed = false
  private rejectStartup!: (error: Error) => void
  private killTimer: NodeJS.Timeout | undefined
  private publishing = 0

  constructor(
    selection: Selection,
    lifetime: vscode.CancellationToken,
    private readonly output: vscode.OutputChannel,
    private readonly changed: () => void,
  ) {
    const [subcommand, ...args] = selection.args
    if (subcommand !== 'lsp') {
      throw new Error(`Tola cannot derive the development command from \`${subcommand}\``)
    }
    if (lifetime.isCancellationRequested) throw new vscode.CancellationError()
    this.child = spawn(selection.command, ['dev', ...args, '--watch=true', '--interface=127.0.0.1'], {
      cwd: selection.folder.uri.fsPath,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    output.appendLine(`Tola starts the development server for ${selection.configurationFile}`)
    let log = ''
    this.ready = new Promise<URL>((resolve, reject) => {
      this.rejectStartup = reject
      const collect = (chunk: Buffer) => {
        const text = chunk.toString()
        output.append(text)
        log = (log + text).slice(-8192)
        const serving = SERVING_LINE.exec(log)?.[1]
        if (serving && !this.stopped && !this.address) {
          this.address = new URL(serving)
          resolve(this.address)
        }
        changed()
      }
      this.child.stdout?.on('data', collect)
      this.child.stderr?.on('data', collect)
      this.child.once(
        'error',
        (error) => {
          output.appendLine(`Development server startup failed: ${error.message}`)
          reject(
            new Error(
              'Tola could not start the development server. Open Tola output for details and run Start Preview again',
            ),
          )
        },
      )
    })
    this.exited = new Promise<void>((resolve) => {
      this.child.once('close', (code, signal) => {
        output.appendLine(`Development server exited (${signal ?? `exit ${code ?? 'unknown'}`})`)
        this.closed = true
        this.stopped = true
        this.cancellation.abort()
        this.lifetime.dispose()
        clearTimeout(this.killTimer)
        this.rejectStartup(
          new Error(
            'Tola development server stopped before serving. Open Tola output for details and run Start Preview again',
          ),
        )
        changed()
        resolve()
      })
    })
    this.lifetime = lifetime.onCancellationRequested(() => this.stop())
  }

  get active(): boolean {
    return !this.stopped
  }

  get running(): boolean {
    return !this.stopped && this.address !== undefined
  }

  readPublication(source?: vscode.Uri, revision?: string): Promise<Publication> {
    return this.request('GET', source, revision)
  }

  get isPublishing(): boolean {
    return this.publishing > 0
  }

  async publish(source?: vscode.Uri): Promise<Publication> {
    this.publishing += 1
    this.changed()
    try {
      return await this.request('POST', source)
    } finally {
      this.publishing -= 1
      this.changed()
    }
  }

  private async request(
    method: 'GET' | 'POST',
    source?: vscode.Uri,
    revision?: string,
  ): Promise<Publication> {
    const address = await this.ready
    this.ensureActive()
    const url = new URL('/_tola/preview', address)
    if (source) url.searchParams.set('source', source.toString())
    if (revision) url.searchParams.set('revision', revision)
    let response: Response
    let publication: Publication & { error?: string }
    try {
      response = await fetch(url, {
        method,
        headers: { 'X-Tola-Preview': '1' },
        signal: this.cancellation.signal,
      })
      publication = await response.json() as Publication & { error?: string }
    } catch (error) {
      this.ensureActive()
      this.output.appendLine(`Development publication request failed: ${String(error)}`)
      throw new Error(
        'Tola could not read the development publication. Open Tola output or run Open Preview again',
      )
    }
    this.ensureActive()
    if (!response.ok) {
      throw new Error(publication.error ?? 'Tola could not publish the saved site; no new preview was opened')
    }
    return publication
  }

  ensureActive(): void {
    if (this.stopped) throw new vscode.CancellationError()
  }

  stop(): void {
    if (this.stopped) return
    this.stopped = true
    this.cancellation.abort()
    this.rejectStartup(new vscode.CancellationError())
    this.lifetime.dispose()
    if (this.closed) return
    // SIGINT lets Tola cancel its build and stop the generators it owns before exiting.
    this.child.kill('SIGINT')
    this.killTimer = setTimeout(() => {
      if (!this.closed) this.child.kill('SIGKILL')
    }, 5000)
    this.killTimer.unref()
  }
}
