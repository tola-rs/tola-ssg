import * as path from 'node:path'
import * as vscode from 'vscode'

import { PreviewServer, type PublishedRoute } from './preview-server.ts'
import { containsFile } from './selection.ts'
import type { Selection } from './selection.ts'
import type { Route, SiteRoute } from './routes.ts'
import type { SourceCheckStatus } from './sites.ts'

export interface PreviewSite {
  readonly selection: Selection
  readonly lifetime: vscode.CancellationToken
  /** The state of the language client that answers this site's own checks. */
  readonly state: () => 'starting' | 'ready' | 'stopped'
  readonly check: () => SourceCheckStatus['state']
  readonly diagnostics: () => number
  /** The routes the site's own check realizes this file at, including unsaved text. */
  readonly routes: (uri: vscode.Uri) => Promise<Route[]>
  readonly siteRoutes: () => Promise<SiteRoute[]>
}

export interface PreviewStatus {
  readonly selection: Selection
  readonly state: 'starting' | 'ready' | 'stopped'
}

export class Preview {
  private readonly servers = new Map<string, PreviewServer>()
  private readonly children = new Set<PreviewServer>()
  private readonly status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100)
  private statusRequest = 0
  private closing = false

  constructor(
    private readonly output: vscode.OutputChannel,
    private readonly siteFor: (uri?: vscode.Uri, siteKey?: string) => PreviewSite | undefined,
    private readonly report: (error: unknown) => void,
    private readonly statusFor: (uri?: vscode.Uri) => PreviewStatus | undefined,
  ) {}

  register(): vscode.Disposable {
    return vscode.Disposable.from(
      vscode.commands.registerCommand(
        'tola.startPreview',
        (uri?: vscode.Uri | string) => this.handle(() => this.start(uri)),
      ),
      vscode.commands.registerCommand(
        'tola.openPreview',
        (uri?: vscode.Uri | string, route?: string, siteKey?: string) =>
          this.handle(() => this.open(uri, route, siteKey)),
      ),
      vscode.commands.registerCommand('tola.copyRoute', () => this.handle(() => this.copyRoute())),
      vscode.commands.registerCommand(
        'tola.openPage',
        (uri?: vscode.Uri | string) => this.handle(() => this.openPage(uri)),
      ),
      vscode.window.onDidChangeActiveTextEditor(() => this.refreshStatus()),
      vscode.workspace.onDidSaveTextDocument(() => this.refreshStatus()),
      vscode.languages.onDidChangeDiagnostics(() => this.refreshStatus()),
      { dispose: () => this.status.dispose() },
      {
        dispose: () => {
          void this.close()
        },
      },
    )
  }

  refreshStatus(): void {
    if (this.closing) return
    const request = ++this.statusRequest
    const document = vscode.window.activeTextEditor?.document
    const status = document && this.statusFor(document.uri)
    if (!document || !status) {
      this.status.hide()
      return
    }
    const state = status.state
    if (state !== 'ready') {
      this.status.text = state === 'starting'
        ? '$(sync~spin) Tola · service starting'
        : '$(warning) Tola · service stopped'
      this.status.tooltip = state === 'starting'
        ? `Tola is starting the language service for ${status.selection.folder.name}.`
        : `Restart the language service for ${status.selection.folder.name}.`
      this.status.command = state === 'stopped'
        ? { title: 'Tola: Restart Language Services', command: 'tola.restart' }
        : undefined
      this.status.show()
      return
    }
    const site = this.siteFor(document.uri)
    if (!site) {
      this.status.hide()
      return
    }
    const server = this.servers.get(this.key(site.selection))
    if (!server?.running) {
      this.showStartAction(document.uri, site)
      return
    }
    void server.readPublication(document.uri).then((publication) => {
      if (request !== this.statusRequest || site.lifetime.isCancellationRequested) return
      const [first] = publication.routes
      this.status.text = first
        ? `$(globe) ${first.route} · ${this.checkState(site)}`
        : `$(globe) ${this.checkState(site)} · No published page`
      if (server.isPublishing) this.status.text += ' · publishing'
      this.status.tooltip = [
        `Source check: ${this.checkState(site)}`,
        ...(server.isPublishing ? ['Preview: publishing saved inputs'] : []),
        first
          ? `Preview: last published ${publication.revision} at ${first.route}`
          : 'Preview: running; the last publication has no page for this source.',
        `Site: ${site.selection.folder.name}`,
      ].join('\n')
      this.status.command = first
        ? { title: 'Tola: Open Preview', command: 'tola.openPreview' }
        : { title: 'Tola: Show Output', command: 'tola.showLog' }
      this.status.show()
    }).catch(() => {
      if (request === this.statusRequest && !site.lifetime.isCancellationRequested) {
        if (!server.running) {
          this.showStartAction(document.uri, site)
          return
        }
        this.status.text = `$(globe) ${this.checkState(site)} · Preview unavailable`
        this.status.tooltip =
          `Tola could not read this site's publication. Run Open Preview or Show Output.\nSite: ${site.selection.folder.name}`
        this.status.command = { title: 'Tola: Show Output', command: 'tola.showLog' }
        this.status.show()
      }
    })
  }

  private showStartAction(source: vscode.Uri, site: PreviewSite): void {
    const server = this.servers.get(this.key(site.selection))
    this.status.text = `$(globe) ${this.checkState(site)} · Start Preview`
    if (server?.isPublishing) this.status.text += ' · publishing'
    this.status.tooltip = [
      `Source check: ${this.checkState(site)}`,
      server?.isPublishing
        ? 'Preview: starting and publishing saved inputs.'
        : 'Preview: not started; Start Preview publishes the saved site without opening a page.',
      `Site: ${site.selection.folder.name}`,
    ].join('\n')
    this.status.command = { title: 'Tola: Start Preview', command: 'tola.startPreview', arguments: [source] }
    this.status.show()
  }

  private checkState(site: PreviewSite): string {
    const state = site.state()
    if (state === 'starting') return 'service starting'
    if (state === 'stopped') return 'service stopped'
    const check = site.check()
    const count = site.diagnostics()
    return `${check === 'notChecked' ? 'not checked' : check}${
      count ? ` (${count} diagnostic${count === 1 ? '' : 's'})` : ''
    }`
  }

  async start(uri?: vscode.Uri | string): Promise<void> {
    const source = typeof uri === 'string' ? vscode.Uri.parse(uri) : uri
    const site = this.requireSite(source)
    if (!await this.save(site)) return
    const server = this.ensure(site)
    await server.publish()
    this.ensureSaved(site)
    server.ensureActive()
    const address = await server.ready
    this.refreshStatus()
    void vscode.window.showInformationMessage(
      `Tola published ${site.selection.folder.name} at ${address.href}`,
    )
  }

  async open(uri?: vscode.Uri | string, route?: string, siteKey?: string): Promise<void> {
    const source = typeof uri === 'string'
      ? vscode.Uri.parse(uri)
      : uri ?? (siteKey === undefined ? vscode.window.activeTextEditor?.document.uri : undefined)
    if (!source && siteKey === undefined) {
      throw new Error('Tola cannot tell which file to preview because no editor is active')
    }
    const site = this.requireSite(source, siteKey)
    if (!await this.save(site)) return
    const server = this.ensure(site)
    const publication = await server.publish(source)
    this.ensureSaved(site)
    const chosen = await this.chooseRoute(source, publication.routes, route)
    if (!chosen) return
    // A route lens is a checked-source hint; only this publication may authorize its browser path.
    const current = await server.readPublication(source, publication.revision)
    if (!current.routes.some((route) => route.route === chosen)) {
      throw new Error(
        'Tola no longer publishes the selected route; run Preview again. No new preview was opened',
      )
    }
    this.ensureSaved(site)
    server.ensureActive()
    const address = await server.ready
    await this.openUrl(site, new URL(chosen, address))
    this.refreshStatus()
  }

  async openPage(uri?: vscode.Uri | string): Promise<void> {
    const source = typeof uri === 'string'
      ? vscode.Uri.parse(uri)
      : uri ?? vscode.window.activeTextEditor?.document.uri
    if (!source) throw new Error('Tola cannot tell which file to open a page for because no editor is active')
    const site = this.requireSite(source)
    const routes = await site.routes(source)
    if (!routes.length) throw new Error('Tola checks no page for this file; no new preview was opened')
    const chosen = await this.chooseRoute(source, routes, undefined, 'checked')
    if (!chosen) return
    if (!await this.save(site)) return
    const server = this.ensure(site)
    const publication = await server.publish(source)
    this.ensureSaved(site)
    // A checked route names the page; only the publication this call made may open it.
    if (!publication.routes.some((route) => route.route === chosen)) {
      throw new Error(
        'Tola no longer publishes this source at the selected route; run Open Page again. No new preview was opened',
      )
    }
    const current = await server.readPublication(source, publication.revision)
    if (!current.routes.some((route) => route.route === chosen)) {
      throw new Error(
        'Tola no longer publishes the selected route; run Open Page again. No new preview was opened',
      )
    }
    this.ensureSaved(site)
    server.ensureActive()
    const address = await server.ready
    await this.openUrl(site, new URL(chosen, address))
    this.refreshStatus()
  }

  /** Where a published page opens: the external browser by default, an editor tab under `tola.previewTarget = simpleBrowser`. */
  private async openUrl(site: PreviewSite, url: URL): Promise<void> {
    const target = vscode.workspace.getConfiguration('tola', site.selection.folder.uri).get<string>(
      'previewTarget',
      'external',
    )
    switch (target) {
      case 'simpleBrowser':
        await vscode.commands.executeCommand('simpleBrowser.show', url.href)
        return
      case 'external':
        await vscode.env.openExternal(vscode.Uri.parse(url.href))
        return
      default:
        throw new Error(
          `Tola cannot open the preview because \`tola.previewTarget\` is \`${target}\`; set it to \`external\` or \`simpleBrowser\``,
        )
    }
  }

  async copyRoute(): Promise<void> {
    const source = vscode.window.activeTextEditor?.document.uri
    if (!source) {
      throw new Error('Tola cannot tell which file to copy a route for because no editor is active')
    }
    const site = this.requireSite(source)
    const server = this.servers.get(this.key(site.selection))
    if (!server?.running) {
      const choice = await vscode.window.showInformationMessage(
        'Tola has no published preview for this site. Start Preview to inspect its published routes.',
        'Start Preview',
      )
      if (choice === 'Start Preview' && !site.lifetime.isCancellationRequested) {
        await vscode.commands.executeCommand('tola.startPreview', source)
      }
      return
    }
    const publication = await server.readPublication(source)
    const route = await this.chooseRoute(source, publication.routes)
    if (!route) return
    const current = await server.readPublication(source, publication.revision)
    if (!current.routes.some((published) => published.route === route)) {
      throw new Error('Tola no longer publishes this source at the selected route; no route was copied')
    }
    server.ensureActive()
    await vscode.env.clipboard.writeText(route)
    void vscode.window.showInformationMessage(
      `Tola copied the published route ${route}; unsaved edits are not included`,
    )
  }

  private async handle(action: () => Promise<void>): Promise<void> {
    try {
      await action()
    } catch (error) {
      this.report(error)
    }
  }

  private requireSite(uri?: vscode.Uri, siteKey?: string): PreviewSite {
    const site = this.siteFor(uri, siteKey)
    if (this.closing || site?.lifetime.isCancellationRequested) throw new vscode.CancellationError()
    if (!site) {
      throw new Error(
        'Tola cannot preview this file because no site answers for it; open a file inside a Tola site',
      )
    }
    return site
  }

  private dirty(site: PreviewSite): vscode.TextDocument[] {
    const selected = site.selection
    return vscode.workspace.textDocuments.filter((document) => {
      if (!document.isDirty || document.uri.scheme !== 'file') return false
      return document.uri.fsPath === selected.config.fsPath ||
        document.uri.fsPath === selected.configurationFile ||
        selected.inputRoots.some((root) => containsFile(root, document.uri.fsPath))
    })
  }

  private sourceName(site: PreviewSite, document: vscode.TextDocument): string {
    const selected = site.selection
    const filename = document.uri.fsPath === selected.configurationFile
      ? selected.config.fsPath
      : document.uri.fsPath
    return path.relative(selected.inputRoots[0]!, filename).split(path.sep).join('/')
  }

  private async save(site: PreviewSite): Promise<boolean> {
    const dirty = this.dirty(site)
    if (dirty.length) {
      const names = dirty.map((document) => this.sourceName(site, document)).join(', ')
      const choice = await vscode.window.showWarningMessage(
        `Preview uses saved files. Save these inputs for ${site.selection.folder.name}: ${names}?`,
        'Save and Preview',
      )
      if (choice !== 'Save and Preview') return false
      if (this.closing || site.lifetime.isCancellationRequested) throw new vscode.CancellationError()
      for (const document of dirty) {
        if (site.lifetime.isCancellationRequested) throw new vscode.CancellationError()
        if (!await document.save()) {
          throw new Error(
            `Tola could not save \`${
              this.sourceName(site, document)
            }\`; save it before running Preview again. No new preview was opened`,
          )
        }
      }
    }
    this.ensureSaved(site)
    return true
  }

  private ensureSaved(site: PreviewSite): void {
    if (this.closing || site.lifetime.isCancellationRequested) throw new vscode.CancellationError()
    if (this.dirty(site).length) {
      throw new Error(
        'Tola found new unsaved edits in this site; run Preview again to save them. No new preview was opened',
      )
    }
  }

  private async chooseRoute(
    source: vscode.Uri | undefined,
    routes: PublishedRoute[],
    preferred?: string,
    kind: 'checked' | 'published' = 'published',
  ): Promise<string | undefined> {
    const [first] = routes
    if (!first) throw new Error('Tola published no HTML page for this file; no new preview was opened')
    if (preferred !== undefined) {
      if (routes.some((route) => route.route === preferred)) return preferred
      throw new Error(
        `Tola no longer publishes the selected route ${preferred}; refresh Pages or run Open Preview to choose again. No new preview was opened`,
      )
    }
    if (routes.length === 1) return first.route
    const chosen = await vscode.window.showQuickPick(
      routes.map((route) => ({ label: route.route, description: route.output })),
      { title: `Tola: ${kind} pages${source ? ` for ${source.path.split('/').pop() ?? ''}` : ''}` },
    )
    return chosen?.label
  }

  private key(selected: Selection): string {
    return JSON.stringify([
      selected.folder.uri.toString(),
      selected.configurationFile,
      selected.command,
      selected.args,
    ])
  }

  private ensure(site: PreviewSite): PreviewServer {
    if (this.closing || site.lifetime.isCancellationRequested) throw new vscode.CancellationError()
    const key = this.key(site.selection)
    const existing = this.servers.get(key)
    if (existing?.active) return existing
    const server = new PreviewServer(site.selection, site.lifetime, this.output, () => this.refreshStatus())
    this.servers.set(key, server)
    this.children.add(server)
    void server.exited.then(() => {
      if (this.servers.get(key) === server) this.servers.delete(key)
      this.children.delete(server)
    })
    return server
  }

  async stop(selection: Selection): Promise<void> {
    const server = this.servers.get(this.key(selection))
    if (!server) return
    server.stop()
    await server.exited
  }

  async close(): Promise<void> {
    this.closing = true
    this.statusRequest += 1
    const servers = [...this.children]
    for (const server of servers) server.stop()
    await Promise.all(servers.map((server) => server.exited))
    this.servers.clear()
  }
}
