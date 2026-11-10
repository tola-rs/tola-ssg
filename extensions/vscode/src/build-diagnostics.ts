import * as path from 'node:path'
import * as vscode from 'vscode'

import { containsFile, type Selection } from './selection.ts'

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
  readonly range?: { readonly start: Position; readonly end: Position }
}

interface Advice {
  readonly message: string
  readonly location: DiagnosticLocation | null
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
}

function sourceLocation(root: string, location: DiagnosticLocation | null): vscode.Location | undefined {
  if (!location || location.path.startsWith('@')) return undefined
  const filename = path.resolve(root, location.path)
  if (!containsFile(root, filename)) return undefined
  const { range, line } = location
  if (range) {
    const { start, end } = range
    return new vscode.Location(
      vscode.Uri.file(filename),
      new vscode.Range(start.line, start.character, end.line, end.character),
    )
  }
  // The terminal's one-based column counts Unicode scalars. Without an exact span, use the line.
  if (line !== null) return new vscode.Location(vscode.Uri.file(filename), new vscode.Position(line - 1, 0))
  return undefined
}

function atDisplayPath(message: string, location: DiagnosticLocation | null): string {
  if (!location) return message
  const line = location.line === null ? '' : `:${location.line}`
  return `${location.path}${line}: ${message}`
}

export function showBuildDiagnostics(
  collection: vscode.DiagnosticCollection,
  selection: Selection,
  reports: Iterable<BuildDiagnostic>,
): void {
  const root = selection.inputRoots[0]!
  const grouped = new Map<string, [vscode.Uri, vscode.Diagnostic[]]>()
  for (const report of reports) {
    // Custom reports carry display labels; the host's editor path is not in the JSON stream.
    const locate = (location: DiagnosticLocation | null) =>
      report.producer ? undefined : sourceLocation(root, location)
    const source = locate(report.location)
    const uri = source?.uri ?? selection.config
    const lines = [source ? report.message : atDisplayPath(report.message, report.location)]
    for (const note of report.notes) lines.push(`Note: ${note}`)
    const related: vscode.DiagnosticRelatedInformation[] = []
    for (const advice of [...report.help, ...(report.related ?? []), ...report.trace]) {
      const location = locate(advice.location)
      if (location) related.push(new vscode.DiagnosticRelatedInformation(location, advice.message))
      else lines.push(atDisplayPath(advice.message, advice.location))
    }
    const diagnostic = new vscode.Diagnostic(
      source?.range ?? new vscode.Range(0, 0, 0, 0),
      lines.join('\n'),
      severity[report.severity],
    )
    diagnostic.code = report.code
    diagnostic.source = 'tola build'
    if (related.length) diagnostic.relatedInformation = related
    const key = uri.toString()
    const group = grouped.get(key)
    if (group) group[1].push(diagnostic)
    else grouped.set(key, [uri, [diagnostic]])
  }
  const updates: [vscode.Uri, vscode.Diagnostic[] | undefined][] = [...grouped.values()]
  collection.forEach((uri) => {
    if (!grouped.has(uri.toString())) updates.push([uri, undefined])
  })
  collection.set(updates)
}
