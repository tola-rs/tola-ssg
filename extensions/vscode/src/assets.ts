import * as vscode from 'vscode'
import * as path from 'node:path'

/** The site a dropped or pasted file publishes into. */
export interface AssetSite {
  /** The directory the site's configuration sits in, which every asset path is written from. */
  readonly root: string
  /** The site-relative directory the drop copies into. */
  readonly directory: string
  readonly current: () => boolean
}

/** One file a drop or paste brings in, with the name it keeps. */
interface Dropped {
  readonly bytes: Uint8Array
  readonly name: string
}

/** The copy and the call one dropped file becomes. */
interface Prepared {
  readonly text: string
  readonly title: string
  readonly files: vscode.WorkspaceEdit
}

/** Where one dropped file lands in the site. */
interface Placed {
  readonly name: string
  readonly uri: vscode.Uri
  readonly relative: string
  readonly files: vscode.WorkspaceEdit
}

/**
 * Dropping or pasting a file into a source copies it into the site's asset directory and inserts
 * the call that publishes it, so a screenshot becomes an asset of the page rather than a path the
 * site cannot read.
 */
export class Assets implements vscode.DocumentDropEditProvider, vscode.DocumentPasteEditProvider {
  constructor(
    private readonly siteFor: (document: vscode.TextDocument) => AssetSite | undefined,
    private readonly report: (error: unknown) => void,
  ) {}

  register(): vscode.Disposable {
    const selector: vscode.DocumentSelector = [{ scheme: 'file', language: 'typst' }]
    return vscode.Disposable.from(
      vscode.languages.registerDocumentDropEditProvider(selector, this, {
        dropMimeTypes: ['text/uri-list', 'files'],
      }),
      vscode.languages.registerDocumentPasteEditProvider(selector, this, {
        providedPasteEditKinds: [vscode.DocumentDropOrPasteEditKind.Empty],
      }),
    )
  }

  async provideDocumentDropEdits(
    document: vscode.TextDocument,
    _position: vscode.Position,
    dataTransfer: vscode.DataTransfer,
    token: vscode.CancellationToken,
  ): Promise<vscode.DocumentDropEdit[] | undefined> {
    const prepared = await this.prepared(document, dataTransfer, token)
    if (!prepared) return undefined
    const insertion = new vscode.DocumentDropEdit(prepared.text)
    insertion.title = prepared.title
    insertion.additionalEdit = prepared.files
    return [insertion]
  }

  async provideDocumentPasteEdits(
    document: vscode.TextDocument,
    _ranges: readonly vscode.Range[],
    dataTransfer: vscode.DataTransfer,
    _context: vscode.DocumentPasteEditContext,
    token: vscode.CancellationToken,
  ): Promise<vscode.DocumentPasteEdit[] | undefined> {
    const prepared = await this.prepared(document, dataTransfer, token)
    if (!prepared) return undefined
    const insertion = new vscode.DocumentPasteEdit(
      prepared.text,
      prepared.title,
      vscode.DocumentDropOrPasteEditKind.Empty,
    )
    insertion.additionalEdit = prepared.files
    return [insertion]
  }

  /** The copy and the call one transferred file becomes, or `undefined` when it is not one. */
  private async prepared(
    document: vscode.TextDocument,
    dataTransfer: vscode.DataTransfer,
    token: vscode.CancellationToken,
  ): Promise<Prepared | undefined> {
    const site = this.siteFor(document)
    const editor = vscode.window.activeTextEditor
    if (!site || editor?.document !== document) return undefined
    const version = document.version
    const selections = editor.selections
    const current = () =>
      !token.isCancellationRequested && !document.isClosed && document.version === version &&
      site.current() && vscode.window.activeTextEditor === editor &&
      editor.selections.length === selections.length &&
      selections.every((selection, index) =>
        editor.selections[index]!.active.isEqual(selection.active) &&
        editor.selections[index]!.anchor.isEqual(selection.anchor)
      )
    try {
      const dropped = await this.dropped(dataTransfer, token)
      if (!dropped || !current()) return undefined
      const placed = await this.place(site, dropped, token)
      if (!placed || !current()) return undefined
      return {
        text: this.inserted(document, placed),
        title: `Add ${placed.name} to the site's assets`,
        files: placed.files,
      }
    } catch (error) {
      if (current()) this.report(error)
      return undefined
    }
  }

  /** The file the transfer carries, when it carries exactly one readable file. */
  private async dropped(
    dataTransfer: vscode.DataTransfer,
    token: vscode.CancellationToken,
  ): Promise<Dropped | undefined> {
    const file = dataTransfer.get('files')?.asFile?.()
    if (file) return { bytes: await file.data(), name: file.name }

    const list = dataTransfer.get('text/uri-list')
    const [listedUri, ...others] = await list?.asString().then(
      (text) => text.split(/\r?\n/).filter((line) => line.startsWith('file:')),
      () => [],
    ) ?? []
    if (!listedUri || others.length || token.isCancellationRequested) return undefined
    const uri = vscode.Uri.parse(listedUri)
    const bytes = await vscode.workspace.fs
      .readFile(uri)
      .then((contents) => contents, () => undefined)
    return bytes ? { bytes, name: path.basename(uri.fsPath) } : undefined
  }

  /** Where the file lands in the site, with a name no existing file holds. */
  private async place(
    site: AssetSite,
    dropped: Dropped,
    token: vscode.CancellationToken,
  ): Promise<Placed | undefined> {
    const directory = vscode.Uri.file(path.join(site.root, site.directory))
    await vscode.workspace.fs.createDirectory(directory)
    const extension = path.extname(dropped.name)
    const stem = path.basename(dropped.name, extension)
    for (let suffix = 0; suffix < 100; suffix += 1) {
      if (token.isCancellationRequested) return undefined
      const name = suffix === 0 ? dropped.name : `${stem}-${suffix}${extension}`
      const uri = vscode.Uri.joinPath(directory, name)
      const taken = await vscode.workspace.fs.stat(uri).then(
        () => true,
        () => false,
      )
      if (taken) continue
      const files = new vscode.WorkspaceEdit()
      files.createFile(uri, { overwrite: false, contents: dropped.bytes })
      return {
        name,
        uri,
        relative: path.posix.join(site.directory, name),
        files,
      }
    }
    return undefined
  }

  /** The call that publishes the copied file, the caret left where the author types next. */
  private inserted(document: vscode.TextDocument, placed: Placed): string {
    const writer = path.dirname(document.uri.fsPath)
    const read = path.relative(writer, placed.uri.fsPath).split(path.sep).join('/')
    return `#asset("${placed.relative}", read("${read}"))$0`
  }
}
