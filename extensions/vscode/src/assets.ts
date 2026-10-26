import * as vscode from 'vscode'
import * as path from 'node:path'
import { realpath } from 'node:fs/promises'
import { containsFile } from './selection.ts'

/** Declared source file URIs and directory URIs, as resolved by the compiler. */
export interface AssetSources {
  readonly files: readonly string[]
  readonly trees: readonly string[]
}

/** The site a dropped or pasted file publishes into. */
export interface AssetSite {
  /** The physical root the compiler uses for site-relative paths. */
  readonly root: string
  /** The site-relative directory the drop copies into. */
  readonly directory: string
  readonly entry: vscode.Uri
  readonly sources: AssetSources
  readonly generated: readonly string[]
  readonly current: () => boolean
}

/** One file a drop or paste brings in, with the name it keeps. */
interface Dropped {
  readonly name: string
  readonly read: () => PromiseLike<Uint8Array | undefined>
}

/** The copy and the call one dropped file becomes. */
interface Prepared {
  readonly title: string
  readonly files: vscode.WorkspaceEdit
}

interface Placed {
  readonly name: string
  readonly uri: vscode.Uri
  /** Configuration-owned sources need no Bundle declaration. */
  readonly declaration?: string
}

const MAX_ASSET_NAME_BYTES = 255

function assetName(name: string): string {
  const portable = name.replace(/[<>:"/\\|?*\p{Cc}]/gu, '-').replace(/[ .]+$/u, '-') || 'asset'
  const stem = portable.split('.')[0]!.replace(/ +$/u, '')
  return /^(con|prn|aux|nul|conin\$|conout\$|(?:com|lpt)[1-9¹²³])$/i.test(stem) ? `_${portable}` : portable
}

function utf8Prefix(text: string, limit: number): string {
  const { read } = new TextEncoder().encodeInto(text, new Uint8Array(limit))
  return text.slice(0, read)
}

function suffixedName(stem: string, extension: string, suffix: bigint): string {
  // Leave room for a complete UTF-8 scalar from the stem; the suffix survives long extensions.
  const ending = utf8Prefix(`${suffix ? `-${suffix}` : ''}${extension}`, MAX_ASSET_NAME_BYTES - 4)
  let budget = MAX_ASSET_NAME_BYTES - Buffer.byteLength(ending)
  let name = assetName(utf8Prefix(stem, budget) + ending)
  while (Buffer.byteLength(name) > MAX_ASSET_NAME_BYTES) {
    budget -= 1
    name = assetName(utf8Prefix(stem, budget) + ending)
  }
  return name
}

async function resolvedDirectory(directory: string): Promise<string> {
  try {
    return await realpath(directory)
  } catch (error) {
    const parent = path.dirname(directory)
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT' || parent === directory) throw error
    return path.join(await resolvedDirectory(parent), path.basename(directory))
  }
}

function typstString(text: string): string {
  return `"${
    text.replace(/["\\\p{Cc}]/gu, (character) =>
      character === '"' || character === '\\'
        ? `\\${character}`
        : `\\u{${character.charCodeAt(0).toString(16)}}`)
  }"`
}

/**
 * A drop copies bytes into the site's assets and, unless configuration already publishes them,
 * declares them at the Bundle entry. A document body cannot contain an asset declaration.
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
    const insertion = new vscode.DocumentDropEdit('')
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
      '',
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
    const entries: Array<{ document: vscode.TextDocument; version: number; text: string }> = []
    const current = () =>
      !token.isCancellationRequested && !document.isClosed && document.version === version &&
      entries.every((entry) => !entry.document.isClosed && entry.document.version === entry.version) &&
      site.current() && vscode.window.activeTextEditor === editor &&
      editor.selections.length === selections.length &&
      selections.every((selection, index) =>
        editor.selections[index]!.active.isEqual(selection.active) &&
        editor.selections[index]!.anchor.isEqual(selection.anchor)
      )
    try {
      const dropped = await this.dropped(dataTransfer, token)
      if (!dropped || !current()) return undefined
      const placed = await this.place(site, dropped.name, token)
      if (!placed || !current()) return undefined
      if (placed.declaration !== undefined) {
        const physical = await realpath(site.entry.fsPath)
        for (const opened of vscode.workspace.textDocuments) {
          if (token.isCancellationRequested) return undefined
          if (opened.uri.scheme !== 'file') continue
          const version = opened.version
          if (await realpath(opened.uri.fsPath).catch(() => undefined) === physical) {
            entries.push({ document: opened, version, text: opened.getText() })
          }
        }
        if (!entries.length) {
          const opened = await vscode.workspace.openTextDocument(site.entry)
          entries.push({ document: opened, version: opened.version, text: opened.getText() })
        }
        if (!current()) return undefined
        if (entries.some((entry) => entry.text !== entries[0]!.text)) {
          throw new Error(
            'Tola cannot publish while the Bundle entry has conflicting open copies; save or close them first',
          )
        }
      }
      const bytes = await dropped.read()
      if (!bytes || !current()) return undefined
      const files = new vscode.WorkspaceEdit()
      files.createFile(placed.uri, { overwrite: false, contents: bytes })
      if (placed.declaration !== undefined) {
        for (const entry of entries) {
          const newline = entry.document.eol === vscode.EndOfLine.CRLF ? '\r\n' : '\n'
          const start = new vscode.Position(0, entry.text.startsWith('\uFEFF') ? 1 : 0)
          files.insert(entry.document.uri, start, placed.declaration + newline)
        }
      }
      return { title: `Publish ${placed.name} as site asset`, files }
    } catch (error) {
      if (current()) this.report(error)
      return undefined
    }
  }

  /** Bytes are deferred until the destination and any required entry edits have been selected. */
  private async dropped(
    dataTransfer: vscode.DataTransfer,
    token: vscode.CancellationToken,
  ): Promise<Dropped | undefined> {
    const file = dataTransfer.get('files')?.asFile?.()
    if (file) return { read: () => file.data(), name: file.name }

    const list = dataTransfer.get('text/uri-list')
    const [listedUri, ...others] = await list?.asString().then(
      (text) => text.split(/\r?\n/).filter((line) => line.startsWith('file:')),
      () => [],
    ) ?? []
    if (!listedUri || others.length || token.isCancellationRequested) return undefined
    const uri = vscode.Uri.parse(listedUri)
    return {
      name: path.basename(uri.fsPath),
      read: () => vscode.workspace.fs.readFile(uri).then((contents) => contents, () => undefined),
    }
  }

  /** Where the file lands in the site, with a name no existing file holds. */
  private async place(
    site: AssetSite,
    droppedName: string,
    token: vscode.CancellationToken,
  ): Promise<Placed | undefined> {
    const refuse = (): never => {
      throw new Error(
        'Tola cannot publish from `tola.assetDirectory`; choose a portable site directory or a declared asset source',
      )
    }
    const directoryPath = await resolvedDirectory(path.resolve(site.root, site.directory)).catch((error) => {
      if ((error as NodeJS.ErrnoException).code === 'ENAMETOOLONG') return refuse()
      throw error
    })
    const trees = site.sources.trees.map((source) => vscode.Uri.parse(source).fsPath)
    const declaredFiles = site.sources.files.map((source) => vscode.Uri.parse(source).fsPath)
    const tree = trees.find((root) => containsFile(root, directoryPath))
    const within = containsFile(site.root, directoryPath)
    if (
      (!within && !tree &&
        !declaredFiles.some((source) => path.relative(path.dirname(source), directoryPath) === '')) ||
      site.generated.some((root) => containsFile(vscode.Uri.parse(root).fsPath, directoryPath))
    ) refuse()
    const directory = vscode.Uri.file(directoryPath)
    const original = assetName(path.basename(droppedName.replaceAll('\\', '-')))
    const extension = path.extname(original)
    const stem = path.basename(original, extension)
    for (let suffix = 0n;; suffix += 1n) {
      if (token.isCancellationRequested) return undefined
      const name = suffixedName(stem, extension, suffix)
      const uri = vscode.Uri.joinPath(directory, name)
      const configured = tree !== undefined ||
        declaredFiles.some((source) => path.relative(source, uri.fsPath) === '')
      if (!within && !configured) refuse()
      const relative = path.relative(site.root, uri.fsPath).split(path.sep).join('/')
      const output = tree ? path.relative(tree, uri.fsPath).split(path.sep).join('/') : relative
      if (
        (!configured || tree) &&
        output.split('/').some((part) =>
          assetName(part) !== part || Buffer.byteLength(part) > MAX_ASSET_NAME_BYTES
        )
      ) refuse()
      if (!configured && output.split('/')[0]?.toLowerCase() === '_tola') {
        if (output.includes('/')) refuse()
        continue
      }
      const taken = await vscode.workspace.fs.stat(uri).then(() => true, () => false)
      if (taken) continue
      return {
        name,
        uri,
        ...(!configured
          ? {
            declaration: `#asset(${typstString(relative)}, read(${
              typstString('/' + relative)
            }, encoding: none))`,
          }
          : {}),
      }
    }
  }
}
