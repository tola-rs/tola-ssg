import * as path from 'node:path'
import * as vscode from 'vscode'
import {
  CloseAction,
  DefinitionRequest,
  DidChangeConfigurationNotification,
  DidChangeTextDocumentNotification,
  DidCloseTextDocumentNotification,
  DidOpenTextDocumentNotification,
  DidSaveTextDocumentNotification,
  ErrorAction,
  type LanguageClientOptions,
  type ServerOptions,
  State,
} from 'vscode-languageclient/node'
import { Assets, type AssetSite } from './assets.ts'
import { SiteCli } from './site-cli.ts'
import { type PagesSite, PagesView } from './pages-view.ts'
import { type EnterSite, insertNewline } from './enter.ts'
import { SiteTasks } from './tasks.ts'
import {
  configurationUri,
  containsFile,
  isUntitledTypst,
  ownsDocument,
  ownsTypstDocument,
  sameSelection,
  selectDocuments,
  type Selection,
  selectSite,
  type ServedWorkspace,
  serverCommand,
} from './selection.ts'
import {
  isCancellation,
  packageScheme,
  type SiteClient,
  siteDefinitions,
  sitePackageUri,
  VirtualSources,
} from './virtual.ts'
import { checkedRoutes, checkedSiteRoutes } from './routes.ts'
import { LanguageService, TolaClient } from './service.ts'
import type { PreviewSite, PreviewStatus } from './preview.ts'

export interface SourceCheckStatus {
  readonly revision: number
  readonly state: 'notChecked' | 'checking' | 'checked' | 'failed'
}

interface WorkspaceClient extends SiteClient {
  readonly selection: ServedWorkspace
  readonly cancellation: vscode.CancellationTokenSource
  readonly resources: vscode.Disposable[]
  readonly extraDocuments: Set<string>
  /** The untitled Typst buffers this server holds open; the client's own synchronization sends their changes. */
  readonly untitledDocuments: Set<string>
  readonly service: LanguageService
  sourceCheck: SourceCheckStatus
}

type Site = WorkspaceClient & { readonly selection: Selection }

type CancellationScope = {
  token: vscode.CancellationToken
  /** True once cancellation, closure, a version change, or the caller's predicate supersedes it. */
  stale(): boolean
  cancel(): void
  dispose(): void
}

function startFailureMessage(folders: readonly string[]): string {
  const [first] = folders
  return folders.length === 1
    ? `Tola could not start language services for ${first}.`
    : `Tola could not start for ${first} and ${folders.length - 1} other folder${
      folders.length === 2 ? '' : 's'
    }.`
}

/** A path as a flat glob: separators read forward, and glob characters inside a real path stay literal. */
function escapeGlob(path: string): string {
  return path.replaceAll('\\', '/').replace(/[?*[\]{}]/g, (character) => `[${character}]`)
}

/** The untitled document a request is about, from the URI its params carry. */
function untitledDocumentUri(params: unknown): string | undefined {
  if (!params || typeof params !== 'object') return undefined
  const { uri, textDocument } = params as { uri?: unknown; textDocument?: { uri?: unknown } }
  const named = typeof uri === 'string'
    ? uri
    : typeof textDocument?.uri === 'string'
    ? textDocument.uri
    : undefined
  return named !== undefined && vscode.Uri.parse(named).scheme === 'untitled' ? named : undefined
}

/** The formatter settings one folder declares, in the shape a service reads them in. */
function formatterSettings(selected: ServedWorkspace): { printWidth: number; proseWrap: boolean } {
  const settings = vscode.workspace.getConfiguration('tola', selected.folder.uri)
  return {
    printWidth: settings.get<number>('formatterPrintWidth', 120),
    proseWrap: settings.get<boolean>('formatterProseWrap', false),
  }
}

/** Only site selections own publishing features; every served workspace owns a language client. */
export class Sites {
  private readonly workspaces = new Map<string, WorkspaceClient>()
  private readonly pendingWatchers = new Map<string, vscode.FileSystemWatcher>()
  /** The site each open untitled Typst buffer belongs to; the author chooses once, and a declined choice stays unattached. */
  private readonly untitled = new Map<string, Promise<string | undefined>>()
  /** The site every enabled folder selected, as of the latest reconcile: a site is listed before its client starts. */
  private selections = new Map<string, Selection>()
  private queue: Promise<void> = Promise.resolve()
  private closing = false
  private readonly virtual = new VirtualSources(
    (key) => this.clientForWorkspace(key),
    (key) => this.clientWhenReady(key),
  )
  private readonly tasks = new SiteTasks(() => [...this.selections.values()])
  private readonly pages: PagesView
  private readonly siteCli: SiteCli

  constructor(
    private readonly output: vscode.OutputChannel,
    private readonly stopPreview: (selection: Selection) => Promise<void>,
    private readonly changed: () => void,
  ) {
    this.pages = new PagesView(() => this.pagesSites(), output)
    this.siteCli = new SiteCli(output, (error) => this.report(error))
  }

  register(): vscode.Disposable {
    return vscode.Disposable.from(
      this.virtual.register(),
      this.tasks.register(),
      this.pages.register(),
      vscode.commands.registerCommand(
        'tola.vendorPackages',
        () => this.verb((selected) => this.siteCli.vendorPackages(selected)),
      ),
      vscode.commands.registerCommand(
        'tola.editorSetup',
        () => this.verb((selected) => this.siteCli.editorSetup(selected)),
      ),
      vscode.commands.registerCommand('tola.initSite', () => this.initSite()),
      this.siteCli.register(),
      vscode.commands.registerCommand(
        'tola.restart',
        () => this.refresh(true).catch((error) => this.report(error)),
      ),
      vscode.commands.registerCommand('tola.goToDefinition', () => this.goToDefinition()),
      vscode.commands.registerCommand('tola.onEnter', () => insertNewline(this.enterSite())),
      vscode.commands.registerCommand('tola.applyCodeAction', (apply: () => Promise<boolean>) => apply()),
      new Assets((document) => this.assetSite(document), (error) => this.report(error)).register(),
      vscode.commands.registerCommand('tola.build', () => this.build()),
      vscode.commands.registerCommand('tola.revealOutput', () => this.revealOutput()),
      vscode.workspace.onDidChangeWorkspaceFolders(() => this.refreshInBackground()),
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (!event.affectsConfiguration('tola')) return
        // A service that is told the settings changed reads the formatter's width and wrapping
        // again, so no service is restarted for them; a setting that selects the service a folder
        // gets restarts exactly that folder, through the refresh below.
        for (const site of this.workspaces.values()) {
          if (
            event.affectsConfiguration('tola.formatterPrintWidth', site.selection.folder.uri) ||
            event.affectsConfiguration('tola.formatterProseWrap', site.selection.folder.uri)
          ) {
            void this.notifyFormatterSettings(site)
          }
        }
        void this.refresh().catch((error) => this.report(error))
      }),
      vscode.workspace.onDidGrantWorkspaceTrust(() => this.refreshInBackground()),
      vscode.workspace.onDidOpenTextDocument((document) => this.synchronize(document, 'open')),
      vscode.workspace.onDidChangeTextDocument((event) => this.synchronize(event.document, 'change')),
      vscode.workspace.onDidSaveTextDocument((document) => this.synchronize(document, 'save')),
      vscode.workspace.onDidCloseTextDocument((document) => {
        // The decision ends with the buffer, so a reused untitled URI asks for its site again.
        if (isUntitledTypst(document)) this.untitled.delete(document.uri.toString())
        this.synchronize(document, 'close')
      }),
    )
  }

  private clientForWorkspace(key: string): WorkspaceClient | undefined {
    const site = this.workspaces.get(key)
    return !this.closing && site && !site.lifetime.isCancellationRequested && site.client.isRunning() &&
        vscode.workspace.isTrusted &&
        vscode.workspace.getConfiguration('tola', site.selection.folder.uri).get<boolean>('enabled', true)
      ? site
      : undefined
  }

  private clientForSite(key: string): Site | undefined {
    const workspace = this.clientForWorkspace(key)
    return workspace?.selection.kind === 'site' ? workspace as Site : undefined
  }

  /**
   * A package document opened during a transition waits for the workspace client that comes up.
   */
  private async clientWhenReady(key: string): Promise<WorkspaceClient | undefined> {
    const running = this.clientForWorkspace(key)
    if (running) return running
    while (true) {
      const transition = this.queue
      await transition
      if (this.closing) return undefined
      const site = this.clientForWorkspace(key)
      // A transition enqueued while this one ran decides the site, so the wait continues with it.
      if (site || this.queue === transition) return site
    }
  }

  private async answers(document: vscode.TextDocument, selected: ServedWorkspace): Promise<boolean> {
    return isUntitledTypst(document)
      ? await this.untitledSite(document) === selected.folder.uri.toString()
      : ownsDocument(selected, document)
  }

  /** The site an untitled Typst buffer belongs to: the only enabled site, or the author's choice among several. */
  private untitledSite(document: vscode.TextDocument): Promise<string | undefined> {
    const key = document.uri.toString()
    let decision = this.untitled.get(key)
    if (!decision) {
      decision = this.chooseSite(document)
      this.untitled.set(key, decision)
    }
    return decision
  }

  private async chooseSite(document: vscode.TextDocument): Promise<string | undefined> {
    if (this.closing) return undefined
    // Every enabled site of the window's last reconcile, not only the clients already running: a
    // buffer that is open while the services start still chooses among all of them.
    const candidates = [...this.selections.values()].filter((selected) =>
      vscode.workspace.getConfiguration('tola', selected.folder.uri).get<boolean>('enabled', true)
    )
    const [only, ...others] = candidates
    if (!only) return undefined
    if (!others.length) return only.folder.uri.toString()
    const chosen = await vscode.window.showQuickPick(
      candidates.map((selected) => ({
        label: selected.folder.name,
        description: selected.config.fsPath,
        key: selected.folder.uri.toString(),
      })),
      {
        title: 'Tola: choose the site for this untitled Typst buffer',
        placeHolder: 'Its language features answer from the site you choose.',
        ignoreFocusOut: true,
      },
    )
    // A buffer closed while the author was choosing has no site: its decision must not outlive it.
    return document.isClosed ? undefined : chosen?.key
  }

  private clientForDocument(document: vscode.TextDocument): SiteClient | undefined {
    if (document.uri.scheme === packageScheme) return this.virtual.clientForDocument(document)
    if (isUntitledTypst(document)) {
      const uri = document.uri.toString()
      for (const [key, site] of this.workspaces) {
        if (site.untitledDocuments.has(uri) && this.clientForWorkspace(key) === site) return site
      }
      return undefined
    }
    const folder = vscode.workspace.getWorkspaceFolder(document.uri)
    const site = folder && this.clientForWorkspace(folder.uri.toString())
    return site && ownsTypstDocument(site.selection, document) ? site : undefined
  }

  refresh(restart = false): Promise<void> {
    for (const site of this.workspaces.values()) {
      if (restart || site.client.state === State.Starting) site.cancellation.cancel()
    }
    const operation = this.queue.then(() => this.reconcile(restart))
    // Later transitions still run; the caller gets the original rejection.
    this.queue = operation.catch(() => {})
    return operation
  }

  private refreshInBackground(): void {
    void this.refresh().catch((error) => this.report(error))
  }

  /** The site a dropped or pasted file publishes into: its root, and its asset directory. */
  assetSite(document: vscode.TextDocument): AssetSite | undefined {
    const folder = vscode.workspace.getWorkspaceFolder(document.uri)
    const site = folder && this.clientForSite(folder.uri.toString())
    if (!site || !ownsTypstDocument(site.selection, document)) return undefined
    const settings = vscode.workspace.getConfiguration('tola', site.selection.folder.uri)
    return {
      root: path.dirname(site.selection.config.fsPath),
      directory: settings.get<string>('assetDirectory', 'assets'),
      current: () => this.clientForWorkspace(site.key) === site,
    }
  }

  /** The site that answers what Enter means for the active source; a read-only package document answers nothing. */
  enterSite(): EnterSite | undefined {
    const document = vscode.window.activeTextEditor?.document
    if (!document || document.uri.scheme === packageScheme) return undefined
    const site = this.clientForDocument(document)
    return site
      ? {
        client: site.client,
        lifetime: site.lifetime,
        current: () => this.clientForDocument(document) === site,
      }
      : undefined
  }

  /** Tell one running service the formatter settings its folder now declares, which it applies where it stands. */
  private async notifyFormatterSettings(site: WorkspaceClient): Promise<void> {
    try {
      await site.client.sendNotification(DidChangeConfigurationNotification.type, {
        settings: { formatter: formatterSettings(site.selection) },
      })
    } catch (error) {
      // A superseded service has nothing to apply them to.
      if (this.clientForWorkspace(site.key) === site) this.report(error)
    }
  }

  private async commandSite(): Promise<Selection | undefined> {
    const document = vscode.window.activeTextEditor?.document
    const selected = document && this.selectionForUri(document.uri)
    if (selected) return selected
    const candidates = [...this.selections.values()]
    const [only, ...others] = candidates
    if (!only) {
      void vscode.window.showInformationMessage('Open a Tola site folder first.')
      return undefined
    }
    if (!others.length) return only
    const chosen = await vscode.window.showQuickPick(
      candidates.map((selection) => ({
        label: selection.folder.name,
        description: selection.config.fsPath,
        selection,
      })),
      { title: 'Tola: choose the site', ignoreFocusOut: true },
    )
    return chosen?.selection
  }

  private async build(): Promise<void> {
    await this.verb((selected) => this.siteCli.build(selected))
  }

  private async revealOutput(): Promise<void> {
    await this.verb((selected) => this.siteCli.revealOutput(selected))
  }

  private selectionForUri(uri: vscode.Uri): Selection | undefined {
    const folder = vscode.workspace.getWorkspaceFolder(uri)
    if (folder) return this.selections.get(folder.uri.toString())
    if (uri.scheme !== 'file') return undefined
    const owners = [...this.selections.values()].filter((selected) =>
      uri.fsPath === selected.config.fsPath || uri.fsPath === selected.configurationFile ||
      selected.inputRoots.some((root) => containsFile(root, uri.fsPath))
    )
    return owners.length === 1 ? owners[0] : undefined
  }

  statusSite(uri?: vscode.Uri): PreviewStatus | undefined {
    const selected = uri && this.selectionForUri(uri)
    if (!selected) return undefined
    const workspace = this.workspaces.get(selected.folder.uri.toString())
    return {
      selection: selected,
      state: workspace?.client.state === State.Starting
        ? 'starting'
        : this.clientForSite(selected.folder.uri.toString())
        ? 'ready'
        : 'stopped',
    }
  }

  previewSite(uri?: vscode.Uri, siteKey?: string): PreviewSite | undefined {
    if (siteKey !== undefined) {
      const site = this.clientForSite(siteKey)
      if (
        !site || uri && (uri.scheme !== 'file' ||
            !site.selection.inputRoots.some((root) => containsFile(root, uri.fsPath)) &&
              uri.fsPath !== site.selection.config.fsPath &&
              uri.fsPath !== site.selection.configurationFile)
      ) return undefined
      return this.previewSiteFor(site)
    }
    const source = uri ?? vscode.window.activeTextEditor?.document.uri
    if (!source || source.scheme !== 'file') return undefined
    const selected = this.selectionForUri(source)
    const site = selected && this.clientForSite(selected.folder.uri.toString())
    return site ? this.previewSiteFor(site) : undefined
  }

  /** The preview surface one site offers: its routes, its lifetime, and the state of its language client. */
  private previewSiteFor(site: Site): PreviewSite {
    return {
      selection: site.selection,
      lifetime: site.lifetime,
      state: () =>
        site.client.state === State.Running
          ? 'ready'
          : site.client.state === State.Starting
          ? 'starting'
          : 'stopped',
      check: () => site.sourceCheck.state,
      diagnostics: () => {
        let count = 0
        site.client.diagnostics?.forEach((_uri, diagnostics) => {
          count += diagnostics.length
        })
        return count
      },
      routes: (source) => checkedRoutes(site.client, source, site.lifetime),
      siteRoutes: () => checkedSiteRoutes(site.client, site.lifetime),
    }
  }

  /** The sites the pages view lists: every enabled site whose client answers its own routes. */
  private pagesSites(): PagesSite[] {
    return [...this.workspaces.keys()].flatMap((key) => {
      const site = this.clientForSite(key)
      return site ? [{ name: site.selection.folder.name, site: this.previewSiteFor(site) }] : []
    })
  }

  private async verb(action: (selected: Selection) => Promise<void>): Promise<void> {
    const selected = await this.commandSite()
    if (selected) await action(selected).catch((error) => this.report(error))
  }

  /** `tola init`: create a site in a directory the author picks, with the executable this window configures. */
  private async initSite(): Promise<void> {
    const chosen = await vscode.window.showOpenDialog({
      canSelectFiles: false,
      canSelectFolders: true,
      canSelectMany: false,
      openLabel: 'Create Site Here',
    })
    const [directory] = chosen ?? []
    if (directory) await this.siteCli.initSite(serverCommand(directory, directory.fsPath), directory)
  }

  report(error: unknown): void {
    if (this.closing || isCancellation(error)) return
    const message = error instanceof Error ? error.message : String(error)
    this.output.appendLine(message)
    void vscode.window.showErrorMessage(
      message.startsWith('Tola ') ? message : `Tola: ${message}`,
      'Restart',
      'Show Output',
    ).then((selected) => {
      if (selected === 'Restart') void this.refresh(true).catch((error) => this.report(error))
      if (selected === 'Show Output') this.output.show()
    })
  }

  private scopedCancellation(
    document: vscode.TextDocument,
    obsolete: () => boolean,
    cancellationSources: readonly vscode.CancellationToken[] = [],
  ): CancellationScope {
    const version = document.version
    const cancellation = new vscode.CancellationTokenSource()
    const listeners = cancellationSources.map((source) =>
      source.onCancellationRequested(() => cancellation.cancel())
    )
    return {
      token: cancellation.token,
      stale: () =>
        cancellation.token.isCancellationRequested ||
        document.isClosed || document.version !== version || obsolete(),
      cancel: () => cancellation.cancel(),
      dispose: () => {
        for (const listener of listeners) listener.dispose()
        cancellation.dispose()
      },
    }
  }

  /** Whether this client sends the document's changes: its own sources and configuration, or an untitled Typst buffer it opened. */
  private synchronizes(site: WorkspaceClient, document: vscode.TextDocument): boolean {
    return isUntitledTypst(document)
      ? site.untitledDocuments.has(document.uri.toString())
      : ownsDocument(site.selection, document)
  }

  /** Whether this client answers feature requests for the document, waiting for the site an untitled buffer is still choosing. */
  private async attached(site: WorkspaceClient, document: vscode.TextDocument): Promise<boolean> {
    if (!isUntitledTypst(document)) return ownsDocument(site.selection, document)
    const uri = document.uri.toString()
    if (!site.untitledDocuments.has(uri) && await this.untitled.get(uri) !== site.key) return false
    // A chosen client answers once its own open notification has gone out, never before.
    return site.untitledDocuments.has(uri)
  }

  private async semantic<T>(
    site: WorkspaceClient,
    document: vscode.TextDocument,
    token: vscode.CancellationToken,
    action: (token: vscode.CancellationToken) => vscode.ProviderResult<T>,
  ): Promise<T | null | undefined> {
    if (this.clientForWorkspace(site.key) !== site || !await this.attached(site, document)) return null
    const scope = this.scopedCancellation(document, () => this.clientForWorkspace(site.key) !== site, [
      token,
      site.lifetime,
    ])
    try {
      if (scope.stale()) return null
      const result = await action(scope.token)
      return scope.stale() ? null : result
    } catch (error) {
      if (scope.stale() || isCancellation(error)) return null
      throw error
    } finally {
      scope.dispose()
    }
  }

  private options(
    selected: ServedWorkspace,
    site: () => WorkspaceClient,
    sourceWatchers: vscode.FileSystemWatcher[],
  ): LanguageClientOptions {
    return {
      workspaceFolder: selected.folder,
      initializationOptions: { formatter: formatterSettings(selected), sourceCheckStatus: true },
      documentSelector: [
        { scheme: 'file', language: 'typst' },
        ...(selected.kind === 'site'
          ? [
            { scheme: 'file', pattern: escapeGlob(selected.config.fsPath) },
            { scheme: 'file', pattern: escapeGlob(selected.configurationFile) },
          ]
          : []),
        // Untitled Typst buffers are selected by every site, and the middleware routes each one to
        // the site that owns it; the server answers them as its unnamed sources.
        { scheme: 'untitled', language: 'typst' },
      ],
      diagnosticCollectionName: `tola:${selected.folder.uri.toString()}`,
      outputChannel: this.output,
      synchronize: { fileEvents: sourceWatchers },
      errorHandler: {
        error: () => ({ action: ErrorAction.Shutdown, handled: true }),
        closed: () => ({
          action: CloseAction.DoNotRestart,
          handled: true,
        }),
      },
      middleware: {
        didOpen: async (document, next) => {
          if (!await this.answers(document, selected)) return
          // A buffer closed while its site was being chosen must not reach the server at all.
          if (document.isClosed) return
          if (isUntitledTypst(document)) site().untitledDocuments.add(document.uri.toString())
          await next(document)
        },
        didChange: async (event, next) => {
          if (this.synchronizes(site(), event.document)) await next(event)
        },
        didSave: async (document, next) => {
          if (this.synchronizes(site(), document)) await next(document)
        },
        didClose: async (document, next) => {
          if (!this.synchronizes(site(), document)) return
          if (isUntitledTypst(document)) site().untitledDocuments.delete(document.uri.toString())
          await next(document)
        },
        sendRequest: async (type, params, token, next) => {
          const untitled = untitledDocumentUri(params)
          if (untitled === undefined) return next(type, params, token)
          const document = vscode.workspace.textDocuments.find((candidate) =>
            candidate.uri.toString() === untitled
          )
          // A request for a closed buffer, or for one another site answers, gets no result here:
          // answering nothing is how the middleware declines a request.
          if (!document || !await this.attached(site(), document)) return undefined as never
          return next(type, params, token)
        },
        provideCodeLenses: (document, token, next) =>
          selected.kind === 'site'
            ? this.semantic(site(), document, token, (requestToken) => next(document, requestToken))
            : null,
        provideCompletionItem: (document, position, context, token, next) =>
          this.semantic(
            site(),
            document,
            token,
            (requestToken) => next(document, position, context, requestToken),
          ),
        provideHover: (document, position, token, next) =>
          this.semantic(site(), document, token, (requestToken) => next(document, position, requestToken)),
        provideDocumentHighlights: (document, position, token, next) =>
          this.semantic(site(), document, token, (requestToken) => next(document, position, requestToken)),
        provideCodeActions: async (document, range, context, token, next) => {
          const owner = site()
          const version = document.version
          const openDocuments = vscode.workspace.textDocuments.map((document) => ({
            document,
            version: document.version,
          }))
          const actions = await this.semantic(
            owner,
            document,
            token,
            (requestToken) => next(document, range, context, requestToken),
          )
          if (!actions) return actions
          for (const action of actions) {
            if (!('edit' in action) || !action.edit) continue
            const edit = action.edit
            const command = action.command
            const editedDocuments = edit.entries().flatMap(([uri]) => {
              const original = openDocuments.find(({ document }) =>
                document.uri.toString() === uri.toString()
              )
              return original ? [original] : []
            })
            delete action.edit
            // VS Code retains contributed command arguments in the extension host; the edit and
            // ownership closure never cross JSON-RPC, which would lose their object identity.
            action.command = {
              title: action.title,
              command: 'tola.applyCodeAction',
              arguments: [async () => {
                if (
                  this.clientForWorkspace(owner.key) !== owner || owner.lifetime.isCancellationRequested ||
                  document.isClosed || document.version !== version ||
                  editedDocuments.some(({ document, version }) =>
                    document.isClosed || document.version !== version
                  )
                ) {
                  this.report(
                    new Error(
                      'Tola code action is no longer current. Request code actions again; no edits were applied',
                    ),
                  )
                  return false
                }
                const applied = await vscode.workspace.applyEdit(edit)
                if (!applied) {
                  this.report(new Error('Tola could not apply this code action. Request code actions again'))
                  return false
                }
                if (command) {
                  await vscode.commands.executeCommand(command.command, ...(command.arguments ?? []))
                }
                return true
              }],
            }
          }
          return actions
        },
        provideSignatureHelp: (document, position, context, token, next) =>
          this.semantic(
            site(),
            document,
            token,
            (requestToken) => next(document, position, context, requestToken),
          ),
        provideDefinition: async (document, position, token, next) => {
          const owner = site()
          const result = await this.semantic(
            owner,
            document,
            token,
            (requestToken) => next(document, position, requestToken),
          )
          return result && siteDefinitions(result, owner.key)
        },
        provideReferences: async (document, position, context, token, next) => {
          const owner = site()
          const result = await this.semantic(
            owner,
            document,
            token,
            (requestToken) => next(document, position, context, requestToken),
          )
          return result &&
            result.map((location) =>
              new vscode.Location(sitePackageUri(location.uri, owner.key), location.range)
            )
        },
        handleDiagnostics: (uri, diagnostics, next) => {
          // A superseded client's diagnostics never reach the editor.
          const owner = site()
          if (this.clientForWorkspace(owner.key) !== owner) return
          next(uri, diagnostics)
        },
      },
    }
  }

  private synchronize(document: vscode.TextDocument, change: 'open' | 'change' | 'save' | 'close'): void {
    if (document.uri.scheme !== 'file') return
    for (const site of this.workspaces.values()) {
      // The client's own middleware sends what this site owns; the mirror carries the rest of its inputs.
      if (this.clientForWorkspace(site.key) !== site || ownsDocument(site.selection, document)) continue
      if (
        !(site.selection.kind === 'site' && document.uri.fsPath === site.selection.configurationFile) &&
        !site.selection.inputRoots.some((root) => containsFile(root, document.uri.fsPath))
      ) continue
      const uri = document.uri.toString()
      const converter = site.client.code2ProtocolConverter
      const notifications: Promise<void>[] = []
      if (change === 'close') {
        if (site.extraDocuments.delete(uri)) {
          notifications.push(
            site.client.sendNotification(
              DidCloseTextDocumentNotification.type,
              converter.asCloseTextDocumentParams(document),
            ),
          )
        }
      } else {
        if (!site.extraDocuments.has(uri)) {
          site.extraDocuments.add(uri)
          notifications.push(
            site.client.sendNotification(
              DidOpenTextDocumentNotification.type,
              converter.asOpenTextDocumentParams(document),
            ),
          )
        } else if (change === 'change') {
          notifications.push(
            site.client.sendNotification(
              DidChangeTextDocumentNotification.type,
              converter.asChangeTextDocumentParams(document),
            ),
          )
        }
        if (change === 'save') {
          notifications.push(
            site.client.sendNotification(
              DidSaveTextDocumentNotification.type,
              converter.asSaveTextDocumentParams(document),
            ),
          )
        }
      }
      void Promise.all(notifications).catch((error) => {
        if (this.clientForWorkspace(site.key) === site) this.report(error)
      })
    }
  }

  private watchConfig(uri: vscode.Uri): vscode.FileSystemWatcher {
    const watcher = vscode.workspace.createFileSystemWatcher(
      new vscode.RelativePattern(path.dirname(uri.fsPath), '*'),
    )
    const changed = (changedUri: vscode.Uri) => {
      if (changedUri.toString() === uri.toString()) this.refreshInBackground()
    }
    watcher.onDidCreate(changed)
    watcher.onDidChange(changed)
    watcher.onDidDelete(changed)
    this.output.appendLine(`Watching ${uri.fsPath}`)
    return watcher
  }

  private async stop(site: WorkspaceClient): Promise<void> {
    site.cancellation.cancel()
    this.virtual.forget(site)
    for (const resource of site.resources) resource.dispose()
    try {
      const stopped = await Promise.allSettled([
        site.client.dispose(),
        site.selection.kind === 'site' ? this.stopPreview(site.selection) : Promise.resolve(),
      ])
      const failed = stopped.find((completion) => completion.status === 'rejected')
      if (failed?.status === 'rejected') throw failed.reason
    } finally {
      await site.service.close()
      site.cancellation.dispose()
      site.extraDocuments.clear()
      site.untitledDocuments.clear()
    }
  }

  private async start(selected: ServedWorkspace): Promise<void> {
    const key = selected.folder.uri.toString()
    const resources: vscode.Disposable[] = []
    let site: WorkspaceClient | undefined
    try {
      const cancellation = new vscode.CancellationTokenSource()
      resources.push(cancellation)
      const service = new LanguageService(selected, this.output, () => {
        if (!this.closing && this.workspaces.get(key) === site) {
          this.report(new Error(`Tola language services stopped for ${selected.folder.name}.`))
        }
      })
      const server: ServerOptions = () => Promise.resolve(service.start(cancellation.token))
      const sourceWatchers: vscode.FileSystemWatcher[] = []
      for (const root of selected.inputRoots) {
        const watcher = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(root, '**/*'))
        sourceWatchers.push(watcher)
        resources.push(watcher)
      }
      if (
        selected.kind === 'site' &&
        !selected.inputRoots.some((root) => containsFile(root, selected.configurationFile))
      ) {
        const watcher = vscode.workspace.createFileSystemWatcher(
          new vscode.RelativePattern(
            path.dirname(selected.configurationFile),
            path.basename(selected.configurationFile),
          ),
        )
        sourceWatchers.push(watcher)
        resources.push(watcher)
      }
      // The client id is the section its own `trace.server` setting is read from, so it stays
      // stable rather than naming the folder.
      const client = new TolaClient(
        'tola',
        `Tola (${selected.folder.name})`,
        server,
        this.options(selected, () => site!, sourceWatchers),
      )
      // Pull reports live in the library's private collection; push keeps this client's public
      // diagnostic collection authoritative for both Problems and Tola's status counts.
      client.registerFeature({
        fillClientCapabilities(capabilities) {
          if (capabilities.textDocument) delete capabilities.textDocument.diagnostic
        },
        initialize() {},
        getState() {
          return { kind: 'static' }
        },
        clear() {},
      })
      site = {
        key,
        selection: selected,
        client,
        cancellation,
        lifetime: cancellation.token,
        resources,
        extraDocuments: new Set(),
        untitledDocuments: new Set(),
        service,
        sourceCheck: { revision: -1, state: 'notChecked' },
      }
      if (selected.kind === 'site') resources.push(this.watchConfig(selected.config))
      resources.push(
        client.onNotification('tola/sourceCheckStatus', (status: SourceCheckStatus) => {
          if (
            this.workspaces.get(key) !== site || site!.lifetime.isCancellationRequested ||
            status.revision < site!.sourceCheck.revision
          ) return
          site!.sourceCheck = status
          if (
            client.state === State.Running && (status.state === 'checked' || status.state === 'failed')
          ) this.pages.refresh()
          else this.pages.updateCheckMessage()
          this.changed()
        }),
        client.onDidChangeState((event) => {
          if (event.newState === State.Stopped) {
            site!.service.detachStartup()
            cancellation.cancel()
            this.virtual.forget(site!)
            this.pages.refresh()
          }
          this.changed()
          if (this.workspaces.get(key) !== site || site!.lifetime.isCancellationRequested) return
          if (
            event.newState === State.Running &&
            (site!.sourceCheck.state === 'checked' || site!.sourceCheck.state === 'failed')
          ) this.pages.refresh()
          else this.pages.updateCheckMessage()
        }),
      )
      this.workspaces.set(key, site)
      await client.start()
      service.ready()
      if (this.closing || cancellation.token.isCancellationRequested) {
        throw new vscode.CancellationError()
      }
      for (const document of vscode.workspace.textDocuments) this.synchronize(document, 'open')
      this.virtual.ready(site)
      this.output.appendLine(`Started Tola language service for ${selected.folder.name}.`)
    } catch (error) {
      const cancelled = this.closing || site?.service.stopRequested
      this.workspaces.delete(key)
      if (site) {
        try {
          await this.stop(site)
        } catch (stopError) {
          this.output.appendLine(
            `Tola could not dispose the language service for ${selected.folder.name}: ${String(stopError)}`,
          )
        }
      } else {
        for (const resource of resources) resource.dispose()
      }
      this.changed()
      if (cancelled) throw new vscode.CancellationError()
      throw error
    }
  }

  private async reconcile(restart: boolean): Promise<void> {
    if (this.closing) return
    const byFolder = new Map<string, ServedWorkspace>()
    const siteSelections = new Map<string, Selection>()
    const failures: string[] = []
    for (const watcher of this.pendingWatchers.values()) watcher.dispose()
    this.pendingWatchers.clear()
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      try {
        const config = configurationUri(folder)
        if (!config) continue
        const selected = await selectSite(folder, config)
        const key = folder.uri.toString()
        if (selected) {
          byFolder.set(key, selected)
          siteSelections.set(key, selected)
        } else {
          this.pendingWatchers.set(key, this.watchConfig(config))
          byFolder.set(key, selectDocuments(folder))
        }
      } catch (error) {
        failures.push(folder.name)
        this.output.appendLine(`Tola could not select a site for ${folder.name}: ${String(error)}`)
      }
    }
    this.selections = siteSelections
    // The palette, the task list, and the pages view follow the enabled sites: a window without
    // one offers none of them.
    void vscode.commands.executeCommand('setContext', 'tola.site', siteSelections.size > 0)
    this.pages.refresh()
    for (const [key, site] of this.workspaces) {
      const selected = byFolder.get(key)
      if (
        restart || !selected || this.clientForWorkspace(key) !== site ||
        !sameSelection(site.selection, selected)
      ) {
        this.workspaces.delete(key)
        try {
          await this.stop(site)
        } catch (error) {
          byFolder.delete(key)
          failures.push(site.selection.folder.name)
          this.output.appendLine(
            `Tola could not stop its language service for ${site.selection.folder.name}: ${String(error)}`,
          )
        }
      }
    }
    for (const [key, selected] of byFolder) {
      if (this.closing || this.workspaces.has(key)) continue
      try {
        await this.start(selected)
      } catch (error) {
        if (isCancellation(error)) continue
        failures.push(selected.folder.name)
        this.output.appendLine(
          `Tola could not start its language service for ${selected.folder.name}: ${String(error)}`,
        )
      }
    }
    this.pages.refresh()
    this.changed()
    if (failures.length) throw new Error(startFailureMessage(failures))
  }

  /** Navigation answers through Tola, so the result is one site's own definitions. */
  private async goToDefinition(): Promise<void> {
    const editor = vscode.window.activeTextEditor
    const session = editor && this.clientForDocument(editor.document)
    if (!editor || !session) {
      void vscode.window.showInformationMessage('Open a Typst document from an enabled Tola site first.')
      return
    }
    const document = editor.document
    const position = editor.selection.active
    const scope = this.scopedCancellation(document, () =>
      vscode.window.activeTextEditor !== editor ||
      !editor.selection.active.isEqual(position) || this.clientForDocument(document) !== session, [
      session.lifetime,
    ])
    const cancelIfStale = () => {
      if (scope.stale()) scope.cancel()
    }
    const listeners = [
      vscode.window.onDidChangeActiveTextEditor(cancelIfStale),
      vscode.window.onDidChangeTextEditorSelection(cancelIfStale),
      vscode.workspace.onDidChangeTextDocument(cancelIfStale),
      vscode.workspace.onDidCloseTextDocument(cancelIfStale),
    ]
    try {
      const locations = document.uri.scheme === packageScheme
        ? await this.virtual.provideDefinition(document, position, scope.token)
        : await session.client.getFeature(DefinitionRequest.method).getProvider(document)?.provideDefinition(
          document,
          position,
          scope.token,
        )
      if (scope.stale()) return
      const entries = !locations ? [] : Array.isArray(locations) ? locations : [locations]
      if (!entries.length) {
        void vscode.window.showInformationMessage('No Tola definition at this position.')
        return
      }
      const targets = entries.map((location) =>
        'targetUri' in location
          ? new vscode.Location(location.targetUri, location.targetSelectionRange ?? location.targetRange)
          : location
      )
      await vscode.commands.executeCommand(
        'editor.action.goToLocations',
        document.uri,
        position,
        targets,
        'goto',
        'No Tola definition',
      )
    } catch (error) {
      if (!scope.stale() && !isCancellation(error)) throw error
    } finally {
      for (const listener of listeners) listener.dispose()
      scope.dispose()
    }
  }

  async close(): Promise<void> {
    this.closing = true
    this.untitled.clear()
    for (const site of this.workspaces.values()) site.cancellation.cancel()
    await this.queue
    for (const watcher of this.pendingWatchers.values()) watcher.dispose()
    this.pendingWatchers.clear()
    const sites = [...this.workspaces.values()]
    this.workspaces.clear()
    const stopped = await Promise.allSettled(sites.map((site) => this.stop(site)))
    this.virtual.dispose()
    const failures = stopped.filter((result) => result.status === 'rejected').map((result) => result.reason)
    if (failures.length) {
      throw new Error(
        failures.length === 1
          ? 'Tola could not stop in one workspace folder. Open the Tola output channel for details.'
          : `Tola could not stop in ${failures.length} workspace folders. Open the Tola output channel for details.`,
      )
    }
  }
}
