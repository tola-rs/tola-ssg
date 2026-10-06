import * as path from 'node:path'
import * as vscode from 'vscode'
import {
  CompletionRequest,
  DefinitionRequest,
  HoverRequest,
  type LanguageClient,
  LSPErrorCodes,
  RequestType,
  ResponseError,
  SemanticTokensRequest,
  SignatureHelpRequest,
} from 'vscode-languageclient/node'

export const packageScheme = 'tola-package'
const sourceRequest = new RequestType<{ uri: string }, { text: string }, void>('tola/source')
/** The query an editor's package URI carries: the site whose client answers for the document. */
const siteQuery = 'site='

export interface SiteClient {
  readonly key: string
  readonly client: LanguageClient
  readonly lifetime: vscode.CancellationToken
}

export function isCancellation(error: unknown): boolean {
  return error instanceof vscode.CancellationError || error instanceof ResponseError &&
      (error.code === LSPErrorCodes.RequestCancelled || error.code === LSPErrorCodes.ContentModified ||
        error.code === LSPErrorCodes.ServerCancelled)
}

/** The package path the server publishes: `/namespace/name/version/path`, with no empty segment. */
function validatePackagePath(uri: vscode.Uri): void {
  const parts = uri.path.split('/')
  const version = parts[3]
  if (
    parts[0] !== '' || parts.length < 5 || !version ||
    parts.slice(1).some((part) => !part || part === '.' || part === '..' || part.includes('\\')) ||
    !/^\d+\.\d+\.\d+$/.test(version)
  ) {
    throw new Error(`Invalid Tola package URI: ${uri.toString()}`)
  }
}

/** The site an editor's package URI names, or `undefined` when the URI is not a package document. */
function packageSite(uri: vscode.Uri): string | undefined {
  return uri.scheme === packageScheme && !uri.authority && !uri.fragment && uri.query.startsWith(siteQuery)
    ? uri.query.slice(siteQuery.length)
    : undefined
}

/** The site one package document belongs to, from the URI its editor shows. */
function validatePackageUri(uri: vscode.Uri): string {
  const site = packageSite(uri)
  if (site === undefined) throw new Error(`Invalid Tola package URI: ${uri.toString()}`)
  validatePackagePath(uri)
  return site
}

/**
 * The URI the server speaks, from the URI an editor document carries.
 *
 * The editor adds the site to every package document it shows; the server's own contract has no
 * authority, query, or fragment, so the qualifier is dropped at this boundary.
 */
export function serverPackageUri(uri: vscode.Uri): vscode.Uri {
  validatePackageUri(uri)
  return uri.with({ query: '' })
}

/** The URI an editor shows for a reply: a package document names the site that answered it, and any other URI is already the editor's own. */
export function sitePackageUri(uri: vscode.Uri, site: string): vscode.Uri {
  return uri.scheme === packageScheme ? uri.with({ query: `${siteQuery}${site}` }) : uri
}

/** The definitions an editor reads: every package URI they name belongs to the site that answered. */
export function siteDefinitions(
  result: vscode.Definition | vscode.LocationLink[],
  site: string,
): vscode.Definition | vscode.LocationLink[] {
  if (!Array.isArray(result)) return new vscode.Location(sitePackageUri(result.uri, site), result.range)
  const links: vscode.LocationLink[] = []
  const locations: vscode.Location[] = []
  for (const entry of result) {
    if ('targetUri' in entry) links.push({ ...entry, targetUri: sitePackageUri(entry.targetUri, site) })
    else locations.push(new vscode.Location(sitePackageUri(entry.uri, site), entry.range))
  }
  return links.length ? links : locations
}

/** The sentence an unavailable package document reports: the site it belongs to, and what to do about it. */
function unavailableSite(site: string): string {
  const uri = vscode.Uri.parse(site)
  const name = vscode.workspace.getWorkspaceFolder(uri)?.name ?? (path.basename(uri.fsPath) || site)
  return `Tola's language service for ${name} is not running. Enable the folder, run Tola: Restart Language Services, then reopen this file.`
}

/** Read-only providers for tola-package documents, routed to the site each URI names. */
export class VirtualSources
  implements
    vscode.TextDocumentContentProvider,
    vscode.CompletionItemProvider,
    vscode.HoverProvider,
    vscode.DefinitionProvider,
    vscode.SignatureHelpProvider,
    vscode.DocumentSemanticTokensProvider,
    vscode.Disposable {
  // A package document is immutable, so its text is read once per client that serves it.
  private readonly sources = new Map<string, { session: SiteClient; text: string }>()
  private readonly changed = new vscode.EventEmitter<vscode.Uri>()
  private painting: vscode.Disposable | undefined
  readonly onDidChange = this.changed.event

  constructor(
    private readonly clientForSite: (key: string) => SiteClient | undefined,
    private readonly clientWhenReady: (key: string) => Promise<SiteClient | undefined>,
  ) {}

  register(): vscode.Disposable {
    // Select by scheme: provider availability must not depend on language inference order.
    const selector: vscode.DocumentSelector = [{ scheme: packageScheme }]
    return vscode.Disposable.from(
      this,
      vscode.workspace.registerTextDocumentContentProvider(packageScheme, this),
      vscode.workspace.onDidOpenTextDocument((document) => {
        if (document.uri.scheme === packageScheme && document.languageId !== 'typst') {
          void vscode.languages.setTextDocumentLanguage(document, 'typst')
        }
      }),
      vscode.languages.registerCompletionItemProvider(selector, this, '.', '@', '"', ':'),
      vscode.languages.registerHoverProvider(selector, this),
      vscode.languages.registerDefinitionProvider(selector, this),
      vscode.languages.registerSignatureHelpProvider(selector, this, '(', ','),
      vscode.workspace.onDidCloseTextDocument((document) => {
        if (document.uri.scheme === packageScheme) this.sources.delete(document.uri.toString())
      }),
    )
  }

  private current(session: SiteClient): boolean {
    return !session.lifetime.isCancellationRequested && this.clientForSite(session.key) === session
  }

  /** A stopped client's cached sources can never answer again. */
  forget(session: SiteClient): void {
    for (const [uri, cached] of this.sources) {
      if (cached.session === session) this.sources.delete(uri)
    }
  }

  /** Refresh the site's own package documents: a document a previous client rendered answers from the client that just started. */
  ready(session: SiteClient): void {
    if (!this.current(session)) return
    this.registerPainting(session)
    for (const document of vscode.workspace.textDocuments) {
      if (document.uri.scheme === packageScheme && packageSite(document.uri) === session.key) {
        this.changed.fire(document.uri)
      }
    }
  }

  /**
   * Paint package documents from the server's own token legend.
   *
   * The legend is the server's, read from the handshake, so the editor and the server agree on
   * what each token number means.
   */
  private registerPainting(session: SiteClient): void {
    if (this.painting) return
    const provider = session.client.initializeResult?.capabilities.semanticTokensProvider
    if (!provider || !('legend' in provider)) return
    this.painting = vscode.languages.registerDocumentSemanticTokensProvider(
      [{ scheme: packageScheme }],
      this,
      provider.legend,
    )
  }

  provideDocumentSemanticTokens(
    document: vscode.TextDocument,
    token: vscode.CancellationToken,
  ): Promise<vscode.SemanticTokens | undefined> {
    return this.semantic(document, token, async (session, requestToken) => {
      const params = { textDocument: { uri: document.uri.toString() } }
      const result = await session.client.sendRequest(
        SemanticTokensRequest.type,
        this.serverParams(params, document.uri),
        requestToken,
      )
      return result ? new vscode.SemanticTokens(Uint32Array.from(result.data), result.resultId) : undefined
    })
  }

  clientForDocument(document: vscode.TextDocument): SiteClient | undefined {
    return this.clientForSite(validatePackageUri(document.uri))
  }

  /** The params the server reads: the package document's own URI, without the site the editor adds. */
  private serverParams<T extends { textDocument: { uri: string } }>(params: T, uri: vscode.Uri): T {
    return { ...params, textDocument: { ...params.textDocument, uri: serverPackageUri(uri).toString() } }
  }

  private async request<T>(
    session: SiteClient,
    token: vscode.CancellationToken,
    action: (session: SiteClient, token: vscode.CancellationToken) => Promise<T>,
  ): Promise<T> {
    const cancellation = new vscode.CancellationTokenSource()
    const inputs = [token, session.lifetime]
    const listeners = inputs.map((input) => input.onCancellationRequested(() => cancellation.cancel()))
    const check = () => {
      if (
        inputs.some((input) => input.isCancellationRequested) || cancellation.token.isCancellationRequested ||
        !this.current(session)
      ) {
        cancellation.cancel()
        throw new vscode.CancellationError()
      }
    }
    try {
      check()
      const result = await action(session, cancellation.token)
      check()
      return result
    } catch (error) {
      check()
      if (isCancellation(error)) throw new vscode.CancellationError()
      throw error
    } finally {
      for (const listener of listeners) listener.dispose()
      cancellation.dispose()
    }
  }

  private async source(
    uri: vscode.Uri,
    session: SiteClient,
    token: vscode.CancellationToken,
  ): Promise<string> {
    const key = uri.toString()
    const cached = this.sources.get(key)
    if (cached?.session === session) return cached.text
    const source = await session.client.sendRequest(
      sourceRequest,
      { uri: serverPackageUri(uri).toString() },
      token,
    )
    if (token.isCancellationRequested || !this.current(session)) throw new vscode.CancellationError()
    if (!source || typeof source.text !== 'string') {
      throw new Error(
        'Tola could not read this package source. Run Tola: Restart Language Services, then reopen the file.',
      )
    }
    this.sources.set(key, { session, text: source.text })
    return source.text
  }

  async provideTextDocumentContent(uri: vscode.Uri, token: vscode.CancellationToken): Promise<string> {
    const site = validatePackageUri(uri)
    // A document restored while its service is still starting reads from the client that comes up;
    // a site no transition selects answers nothing rather than another site's client.
    const session = await this.clientWhenReady(site)
    if (token.isCancellationRequested) throw new vscode.CancellationError()
    if (!session) throw new Error(unavailableSite(site))
    return this.request(session, token, (owner, requestToken) => this.source(uri, owner, requestToken))
  }

  private async semantic<T>(
    document: vscode.TextDocument,
    token: vscode.CancellationToken,
    action: (session: SiteClient, token: vscode.CancellationToken) => Promise<T | undefined>,
  ): Promise<T | undefined> {
    const session = await this.clientWhenReady(validatePackageUri(document.uri))
    if (!session) return undefined
    const version = document.version
    return this.request(session, token, async (owner, requestToken) => {
      const source = await this.source(document.uri, owner, requestToken)
      if (document.isClosed || document.version !== version) throw new vscode.CancellationError()
      if (source !== document.getText()) {
        // A replaced package binary may have different bytes; refresh the model before interpreting positions.
        this.changed.fire(document.uri)
        return undefined
      }
      const result = await action(owner, requestToken)
      if (document.isClosed || document.version !== version) throw new vscode.CancellationError()
      return result
    })
  }

  provideCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken,
    context: vscode.CompletionContext,
  ): Promise<vscode.CompletionItem[] | vscode.CompletionList | undefined> {
    return this.semantic(document, token, async (session, requestToken) => {
      const client = session.client
      const params = client.code2ProtocolConverter.asCompletionParams(document, position, context)
      const result = await client.sendRequest(
        CompletionRequest.type,
        this.serverParams(params, document.uri),
        requestToken,
      )
      return client.protocol2CodeConverter.asCompletionResult(
        result,
        client.initializeResult?.capabilities.completionProvider?.allCommitCharacters,
        requestToken,
      )
    })
  }

  provideHover(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken,
  ): Promise<vscode.Hover | undefined> {
    return this.semantic(document, token, async (session, requestToken) => {
      const client = session.client
      const params = client.code2ProtocolConverter.asTextDocumentPositionParams(document, position)
      const result = await client.sendRequest(
        HoverRequest.type,
        this.serverParams(params, document.uri),
        requestToken,
      )
      return client.protocol2CodeConverter.asHover(result)
    })
  }

  provideDefinition(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken,
  ): Promise<vscode.Definition | vscode.DefinitionLink[] | undefined> {
    return this.semantic(document, token, async (session, requestToken) => {
      const client = session.client
      const params = client.code2ProtocolConverter.asTextDocumentPositionParams(document, position)
      const response = await client.sendRequest(
        DefinitionRequest.type,
        this.serverParams(params, document.uri),
        requestToken,
      )
      const result = await client.protocol2CodeConverter.asDefinitionResult(response, requestToken)
      return result && siteDefinitions(result, session.key)
    })
  }

  provideSignatureHelp(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken,
    context: vscode.SignatureHelpContext,
  ): Promise<vscode.SignatureHelp | undefined> {
    return this.semantic(document, token, async (session, requestToken) => {
      const client = session.client
      const params = client.code2ProtocolConverter.asSignatureHelpParams(document, position, context)
      const result = await client.sendRequest(
        SignatureHelpRequest.type,
        this.serverParams(params, document.uri),
        requestToken,
      )
      return client.protocol2CodeConverter.asSignatureHelp(result, requestToken)
    })
  }

  dispose(): void {
    this.painting?.dispose()
    this.sources.clear()
    this.changed.dispose()
  }
}
