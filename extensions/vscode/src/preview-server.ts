import { type ChildProcess, spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import * as vscode from 'vscode'

import { type BuildDiagnostic, showBuildDiagnostics } from './build-diagnostics.ts'
import type { Selection } from './selection.ts'

type BuildOutcome = 'succeeded' | 'failed' | 'cancelled'
type BuildStatus = 'starting' | 'building' | BuildOutcome

type DevelopmentEvent = { readonly formatVersion: number } & (
  | { readonly event: 'server-ready'; readonly url: string; readonly revision: string | null }
  | { readonly event: 'build-started'; readonly build: number }
  | {
    readonly event: 'build-finished'
    readonly build: number
    readonly status: BuildOutcome
    readonly published: boolean
    readonly revision: string | null
    readonly diagnostics: readonly BuildDiagnostic[]
  }
  | {
    readonly event: 'hook-finished'
    readonly hook: string | null
    readonly revision: string
    readonly status: BuildOutcome
    readonly diagnostics: readonly BuildDiagnostic[]
  }
  | { readonly event: 'stopped'; readonly diagnostics: readonly BuildDiagnostic[] }
)

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
  private rejectStartup!: (error: Error) => void
  private publishing = 0
  private build: BuildStatus = 'starting'
  private buildAttempt = 0
  private revision: string | null = null
  private buildReports: readonly BuildDiagnostic[] = []
  private readonly hooks = new Map<string | null, {
    readonly status: Exclude<BuildOutcome, 'cancelled'>
    readonly diagnostics: readonly BuildDiagnostic[]
  }>()

  constructor(
    private readonly selection: Selection,
    lifetime: vscode.CancellationToken,
    private readonly output: vscode.OutputChannel,
    private readonly diagnostics: vscode.DiagnosticCollection,
    private readonly changed: (published?: boolean) => void,
  ) {
    const [subcommand, ...args] = selection.args
    if (subcommand !== 'lsp') {
      throw new Error(`Tola cannot derive the development command from \`${subcommand}\``)
    }
    if (lifetime.isCancellationRequested) throw new vscode.CancellationError()
    this.child = spawn(
      selection.command,
      ['dev', ...args, '--watch=true', '--interface=127.0.0.1', '--json'],
      {
        cwd: selection.folder.uri.fsPath,
        shell: false,
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    )
    output.appendLine(`Tola starts the development server for ${selection.configurationFile}`)
    this.child.stderr?.on('data', (chunk: Buffer) => output.append(chunk.toString()))
    const events = createInterface({ input: this.child.stdout!, crlfDelay: Infinity })
    this.ready = new Promise<URL>((resolve, reject) => {
      this.rejectStartup = reject
      events.on('line', (line) => {
        if (this.cancellation.signal.aborted) return
        try {
          const published = this.receive(JSON.parse(line) as DevelopmentEvent)
          if (this.address) resolve(this.address)
          changed(published)
        } catch (error) {
          output.appendLine(`Development event could not be read: ${String(error)}`)
          reject(
            new Error('Tola could not read its development events. Update Tola and restart the preview'),
          )
          this.stop()
        }
      })
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
        this.cancellation.abort()
        this.lifetime.dispose()
        events.close()
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
    if (lifetime.isCancellationRequested) this.stop()
  }

  get active(): boolean {
    return !this.cancellation.signal.aborted
  }

  get running(): boolean {
    return !this.cancellation.signal.aborted && this.address !== undefined
  }

  get publishedRevision(): string | null {
    return this.revision
  }

  private receive(event: DevelopmentEvent): boolean {
    if (event?.formatVersion !== 1 || typeof event.event !== 'string') {
      throw new Error('Unsupported development event format')
    }
    switch (event.event) {
      case 'server-ready':
        if (typeof event.url !== 'string') throw new Error('The server reported no preview URL')
        this.address = new URL(event.url)
        this.setRevision(event.revision)
        break
      case 'build-started':
        if (event.build <= this.buildAttempt) break
        this.buildAttempt = event.build
        this.build = 'building'
        break
      case 'build-finished':
        if (event.build !== this.buildAttempt || this.build !== 'building') break
        if (!['succeeded', 'failed', 'cancelled'].includes(event.status)) {
          throw new Error('The server reported an unknown build outcome')
        }
        this.build = event.status
        this.setRevision(event.revision)
        if (event.status !== 'cancelled') this.buildReports = event.diagnostics
        this.showDiagnostics()
        return event.published === true
      case 'hook-finished':
        if ((event.hook !== null && typeof event.hook !== 'string') || typeof event.revision !== 'string') {
          throw new Error('The server did not identify the completed publication hook')
        }
        if (event.status === 'failed') {
          const action = event.hook === null ? 'After-publish preparation' : `Publication hook ${event.hook}`
          this.output.appendLine(`${action} failed for ${event.revision}`)
        }
        if (event.revision !== this.revision || event.status === 'cancelled') break
        this.hooks.set(event.hook, { status: event.status, diagnostics: event.diagnostics })
        this.showDiagnostics()
        break
      case 'stopped':
        if (event.diagnostics.length) {
          this.buildReports = event.diagnostics
          this.hooks.clear()
          this.showDiagnostics()
        }
        this.cancellation.abort()
        this.rejectStartup(
          new Error('Tola stopped before its preview became available. Open Tola output for details'),
        )
        this.lifetime.dispose()
        break
    }
    return false
  }

  private setRevision(revision: string | null): void {
    if (revision !== null && typeof revision !== 'string') {
      throw new Error('The server reported an invalid publication revision')
    }
    if (revision !== this.revision) this.hooks.clear()
    this.revision = revision
  }

  private showDiagnostics(): void {
    const hooks = Array.from(this.hooks.values(), (hook) => hook.diagnostics)
    showBuildDiagnostics(this.diagnostics, this.selection, [this.buildReports, ...hooks].flat())
  }

  get buildStatus(): BuildStatus {
    return this.build
  }

  get buildDescription(): string {
    if (this.build === 'failed') {
      return this.revision
        ? 'Build failed; preview still shows the previous publication.'
        : 'Build failed; no preview has been published.'
    }
    const descriptions: Record<Exclude<BuildStatus, 'failed'>, string> = {
      cancelled: 'The latest build was cancelled.',
      building: 'Building saved inputs.',
      succeeded: 'Saved inputs built successfully.',
      starting: 'Waiting for the first build.',
    }
    return descriptions[this.build]
  }

  get hookFailure(): string | undefined {
    for (const [hook, { status }] of this.hooks) {
      if (status === 'failed') {
        const action = hook === null ? 'After-publish preparation' : `Publication hook ${hook}`
        return `${action} failed; the page remains published.`
      }
    }
    return undefined
  }

  readPublication(source?: vscode.Uri, revision?: string, fragment?: string): Promise<Publication> {
    return this.request('GET', source, revision, fragment)
  }

  get isPublishing(): boolean {
    return this.publishing > 0 || this.build === 'building'
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
    fragment?: string,
  ): Promise<Publication> {
    const address = await this.ready
    this.ensureActive()
    const url = new URL('/_tola/preview', address)
    if (source) url.searchParams.set('source', source.toString())
    if (revision) url.searchParams.set('revision', revision)
    if (fragment !== undefined) url.searchParams.set('fragment', fragment)
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
    if (this.cancellation.signal.aborted) throw new vscode.CancellationError()
  }

  stop(): void {
    if (this.cancellation.signal.aborted) return
    this.cancellation.abort()
    this.rejectStartup(new vscode.CancellationError())
    this.lifetime.dispose()
    this.changed()
    // SIGINT lets Tola cancel its build and stop the generators it owns before exiting.
    this.child.kill('SIGINT')
  }
}
