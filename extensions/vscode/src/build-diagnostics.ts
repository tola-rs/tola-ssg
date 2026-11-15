import { createHash } from 'node:crypto'
import * as path from 'node:path'
import * as vscode from 'vscode'

import { containsFile, sameSelection, type Selection } from './selection.ts'

const severity = {
  error: vscode.DiagnosticSeverity.Error,
  warning: vscode.DiagnosticSeverity.Warning,
  info: vscode.DiagnosticSeverity.Information,
  hint: vscode.DiagnosticSeverity.Hint,
}

interface Position {
  readonly line: number
  readonly character: number
}

interface DiagnosticLocation {
  readonly path: string
  readonly line: number | null
  readonly column?: number | null
  readonly range?: { readonly start: Position; readonly end: Position }
  readonly source_lines?: readonly {
    readonly line: number
    readonly start_character: number
    readonly text: string
  }[]
}

interface Advice {
  readonly message: string
  readonly location: DiagnosticLocation | null
}

interface EditorFile {
  readonly path: string
  readonly sha256?: string
}

export interface BuildDiagnostic {
  readonly code: string
  readonly severity: keyof typeof severity
  readonly producer?: { readonly name: string | null; readonly code: string | null }
  readonly message: string
  readonly location: DiagnosticLocation | null
  readonly notes: readonly string[]
  readonly help: readonly Advice[]
  readonly related?: readonly Advice[]
  readonly trace: readonly Advice[]
  readonly editor?: {
    readonly primary?: EditorFile
    readonly help?: readonly (EditorFile | null)[]
    readonly related?: readonly (EditorFile | null)[]
    readonly trace?: readonly (EditorFile | null)[]
  }
}

type BuildSource = 'build' | 'preview'
type Snapshot = ReadonlyMap<string, { document: vscode.TextDocument; version: number; dirty: boolean }>

export function savedDocuments(): Snapshot {
  return new Map(vscode.workspace.textDocuments.map((document) => [
    document.uri.toString(), { document, version: document.version, dirty: document.isDirty },
  ]))
}

export interface BuildReports {
  set(reports: readonly BuildDiagnostic[], snapshot: Snapshot): void
  clear(): void
}

interface Batch {
  readonly selection: Selection
  reports: readonly BuildDiagnostic[]
  snapshot: Snapshot
}

interface FileText {
  readonly text: string
  readonly sha256: string
  readonly sourceLines: readonly number[]
  readonly editorLines: readonly number[]
}

function fileUri(
  selection: Selection,
  location: DiagnosticLocation | null,
  file: EditorFile | null | undefined,
  custom: boolean,
): vscode.Uri | undefined {
  if (!location) return undefined
  if (file) {
    if (!path.isAbsolute(file.path)) return undefined
    return file.path === selection.configurationFile ? selection.config : vscode.Uri.file(file.path)
  }
  if (custom || location.path.startsWith('@')) return undefined
  const root = selection.inputRoots[0]!
  const filename = path.resolve(root, location.path)
  return containsFile(root, filename) ? vscode.Uri.file(filename) : undefined
}

function lineStarts(text: string, breaks: RegExp): number[] {
  return [0, ...Array.from(text.matchAll(breaks), (match) => match.index + match[0].length)]
}

function editorPosition(lines: readonly number[], offset: number): vscode.Position {
  let first = 0
  let last = lines.length
  while (first + 1 < last) {
    const middle = Math.floor((first + last) / 2)
    if (lines[middle]! <= offset) first = middle
    else last = middle
  }
  return new vscode.Position(first, offset - lines[first]!)
}

function editorRange(text: FileText, location: DiagnosticLocation): vscode.Range | undefined {
  if (location.source_lines?.some((line) => {
    const offset = text.sourceLines[line.line - 1]
    return offset === undefined || !text.text.startsWith(line.text, offset + line.start_character)
  })) return undefined

  const position = ({ line, character }: Position) => {
    const start = text.sourceLines[line]
    if (start === undefined || !Number.isSafeInteger(character) || character < 0) return undefined
    const offset = start + character
    if (offset > text.text.length || offset >= (text.sourceLines[line + 1] ?? Infinity)) return undefined
    return editorPosition(text.editorLines, offset)
  }
  if (location.range) {
    const start = position(location.range.start)
    const end = position(location.range.end)
    return start && end && !end.isBefore(start) ? new vscode.Range(start, end) : undefined
  }
  const start = position({ line: (location.line ?? 1) - 1, character: 0 })
  return start && new vscode.Range(start, start)
}

function atDisplayPath(message: string, location: DiagnosticLocation | null): string {
  if (!location) return message
  const line = location.line === null ? '' : `:${location.line}`
  const column = line && location.column != null ? `:${location.column}` : ''
  return `${location.path}${line}${column}: ${message}`
}

function displayMessage(report: BuildDiagnostic): string {
  return report.producer ? report.message : report.message.replace(/^panicked with: /, '')
}

export function buildErrorMessage(reports: readonly BuildDiagnostic[]): string | undefined {
  const report = reports.find((report) => report.severity === 'error')
  if (!report) return undefined
  const message = atDisplayPath(displayMessage(report), report.location)
  const help = report.help[0]?.message
  if (!help) return message
  const punctuation = /[.!?]$/.test(message) ? '' : '.'
  return `${message}${punctuation} ${help}`
}

function messages(report: BuildDiagnostic): string[] {
  return [
    atDisplayPath(displayMessage(report), report.location),
    ...report.notes.map((note) => `note: ${note}`),
    ...[...report.help, ...(report.related ?? []), ...report.trace].map((advice) =>
      atDisplayPath(advice.message, advice.location)
    ),
  ]
}

function sameProblem(left: vscode.Diagnostic, right: vscode.Diagnostic, message: string): boolean {
  return left.code === right.code && left.severity === right.severity &&
    left.range.isEqual(right.range) &&
    (left.message === message || left.message.startsWith(`${message}\n`))
}

/** The command diagnostics for a site, projected onto the files the commands actually read. */
export class BuildDiagnostics implements vscode.Disposable {
  private readonly batches = new Map<string, Map<BuildSource, Batch>>()
  private readonly collection = vscode.languages.createDiagnosticCollection('tola-build')
  private readonly listeners = vscode.Disposable.from(
    vscode.languages.onDidChangeDiagnostics(() => this.refresh()),
    vscode.workspace.onDidChangeTextDocument(() => this.refresh()),
    vscode.workspace.onDidSaveTextDocument(() => this.refresh()),
    vscode.workspace.onDidCloseTextDocument(() => this.refresh()),
    vscode.workspace.onDidOpenTextDocument(() => this.refresh()),
  )
  private generation = 0
  private displayed = ''
  private disposed = false

  constructor(
    private readonly output: vscode.OutputChannel,
    private readonly sourceReports: (selection: Selection, uri: vscode.Uri) => readonly vscode.Diagnostic[],
  ) {}

  open(selection: Selection, source: BuildSource): BuildReports {
    const key = selection.folder.uri.toString()
    const site = this.batches.get(key) ?? new Map<BuildSource, Batch>()
    const batch: Batch = { selection, reports: [], snapshot: new Map() }
    site.set(source, batch)
    this.batches.set(key, site)
    this.refresh()
    const current = () => this.batches.get(key) === site && site.get(source) === batch
    return {
      set: (reports, snapshot) => {
        if (!current()) return
        batch.reports = reports
        batch.snapshot = snapshot
        for (const report of reports) {
          if (!fileUri(selection, report.location, report.editor?.primary, !!report.producer)) {
            this.output.appendLine(messages(report).join('\n'))
          }
        }
        this.refresh()
      },
      clear: () => {
        if (!current()) return
        site.delete(source)
        if (!site.size) this.batches.delete(key)
        this.refresh()
      },
    }
  }

  clear(selection: Selection, source: BuildSource): void {
    const key = selection.folder.uri.toString()
    const site = this.batches.get(key)
    const batch = site?.get(source)
    if (!site || !batch || !sameSelection(batch.selection, selection)) return
    site.delete(source)
    if (!site.size) this.batches.delete(key)
    this.refresh()
  }

  forget(selection: Selection): void {
    const key = selection.folder.uri.toString()
    const site = this.batches.get(key)
    if (!site) return
    for (const [source, batch] of site) {
      if (sameSelection(batch.selection, selection)) site.delete(source)
    }
    if (!site.size) this.batches.delete(key)
    this.refresh()
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.listeners.dispose()
    this.batches.clear()
    this.collection.dispose()
  }

  private refresh(): void {
    if (this.disposed) return
    const generation = ++this.generation
    const batches = [...this.batches.values()].flatMap((site) => [...site.values()])
    void this.publish(generation, batches).catch((error: unknown) => {
      if (generation === this.generation && !this.disposed) {
        this.output.appendLine(`Tola could not display build diagnostics: ${String(error)}`)
      }
    })
  }

  private async publish(generation: number, batches: readonly Batch[]): Promise<void> {
    const documents = savedDocuments()
    const texts = new Map<string, Promise<FileText | undefined>>()
    const read = (uri: vscode.Uri, document: vscode.TextDocument | undefined) => {
      const key = uri.toString()
      const retained = texts.get(key)
      if (retained) return retained
      const bytes = document
        ? Promise.resolve(Buffer.from(document.getText()))
        : Promise.resolve(vscode.workspace.fs.readFile(uri)).catch(() => undefined)
      const text = bytes.then((bytes) => {
        if (!bytes) return undefined
        const text = Buffer.from(bytes).toString('utf8')
        return {
          text,
          sha256: createHash('sha256').update(bytes).digest('hex'),
          sourceLines: lineStarts(text, /\r\n|[\n\r\v\f\u0085\u2028\u2029]/gu),
          editorLines: lineStarts(text, /\r\n|[\n\r]/gu),
        }
      })
      texts.set(key, text)
      return text
    }

    const entries = await Promise.all(batches.flatMap((batch) => batch.reports.map(async (report) => {
      const locate = async (location: DiagnosticLocation | null, file?: EditorFile | null) => {
        const uri = fileUri(batch.selection, location, file, !!report.producer)
        if (!uri || !location) return undefined
        const key = uri.toString()
        const opened = documents.get(key)
        const started = batch.snapshot.get(key)
        if (opened?.dirty || (opened && started && (
          opened.document !== started.document || opened.version !== started.version || started.dirty
        ))) return undefined
        if (!location.range && location.line === null && !file?.sha256) {
          return new vscode.Location(uri, new vscode.Position(0, 0))
        }
        const text = await read(uri, opened?.document)
        if (!text || (file?.sha256 && text.sha256 !== file.sha256)) return undefined
        const range = editorRange(text, location)
        return range && new vscode.Location(uri, range)
      }
      const source = await locate(report.location, report.editor?.primary)
      if (!source) return undefined
      const message = displayMessage(report)
      const lines = [message, ...report.notes.map((note) => `note: ${note}`)]
      const related: vscode.DiagnosticRelatedInformation[] = []
      for (const [kind, list] of [
        ['help', report.help], ['related', report.related ?? []], ['trace', report.trace],
      ] as const) {
        for (const [index, advice] of list.entries()) {
          const location = await locate(advice.location, report.editor?.[kind]?.[index])
          if (location) related.push(new vscode.DiagnosticRelatedInformation(location, advice.message))
          else lines.push(atDisplayPath(advice.message, advice.location))
        }
      }
      const diagnostic = new vscode.Diagnostic(source.range, lines.join('\n'), severity[report.severity])
      diagnostic.code = report.producer ? report.producer.code ?? undefined : report.code
      diagnostic.source = report.producer?.name ?? 'tola build'
      if (related.length) diagnostic.relatedInformation = related
      return { source, diagnostic, message, selection: batch.selection }
    })))
    if (this.disposed || generation !== this.generation) return

    const grouped = new Map<string, [vscode.Uri, vscode.Diagnostic[]]>()
    for (const entry of entries) {
      if (!entry) continue
      const { source, diagnostic, message, selection } = entry
      if (this.sourceReports(selection, source.uri).some((report) =>
        (report.source === diagnostic.source || diagnostic.source === 'tola build' && report.source === 'tola') &&
        sameProblem(report, diagnostic, message)
      )) continue
      const key = source.uri.toString()
      const group = grouped.get(key) ?? [source.uri, []]
      if (!group[1].some((report) => report.source === diagnostic.source && sameProblem(report, diagnostic, message))) {
        group[1].push(diagnostic)
      }
      grouped.set(key, group)
    }
    const updates = [...grouped.values()]
    const displayed = JSON.stringify(updates)
    if (displayed === this.displayed) return
    this.displayed = displayed
    const changes: [vscode.Uri, vscode.Diagnostic[] | undefined][] = [...updates]
    this.collection.forEach((uri) => {
      if (!grouped.has(uri.toString())) changes.push([uri, undefined])
    })
    this.collection.set(changes)
  }
}
