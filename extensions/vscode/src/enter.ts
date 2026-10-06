import * as vscode from 'vscode'
import { type LanguageClient, RequestType, type TextEdit } from 'vscode-languageclient/node'

/** The site that answers what Enter means at one position in one of its sources. */
export interface EnterSite {
  readonly client: LanguageClient
  readonly lifetime: vscode.CancellationToken
  readonly current: () => boolean
}

interface EnterReply {
  edits?: TextEdit[]
}

const onEnterRequest = new RequestType<{ uri: string; position: vscode.Position }, EnterReply, void>(
  'tola/onEnter',
)

/**
 * Insert what the site's own syntax calls for at the cursor.
 *
 * A site that answers nothing keeps the editor's newline, so the key never does less than VS Code
 * would do on its own.
 */
export async function insertNewline(site: EnterSite | undefined): Promise<void> {
  const editor = vscode.window.activeTextEditor
  const document = editor?.document
  if (!editor || !document || !site) return newline()
  const version = document.version
  const selection = editor.selection
  const current = () =>
    !document.isClosed && document.version === version && site.current() &&
    !site.lifetime.isCancellationRequested && vscode.window.activeTextEditor === editor &&
    editor.selection.active.isEqual(selection.active) && editor.selection.anchor.isEqual(selection.anchor)
  const cancellation = new vscode.CancellationTokenSource()
  const changed = () => {
    if (!current()) cancellation.cancel()
  }
  const listeners = [
    vscode.window.onDidChangeActiveTextEditor(changed),
    vscode.window.onDidChangeTextEditorSelection(changed),
    vscode.workspace.onDidChangeTextDocument(changed),
    vscode.workspace.onDidCloseTextDocument(changed),
    site.lifetime.onCancellationRequested(() => cancellation.cancel()),
  ]
  try {
    const edits = await site.client
      .sendRequest(
        onEnterRequest,
        { uri: document.uri.toString(), position: selection.active },
        cancellation.token,
      )
      .then((reply) => site.client.protocol2CodeConverter.asTextEdits(reply?.edits))
      .catch(() => undefined)
    if (cancellation.token.isCancellationRequested || !current()) return
    if (!edits?.length) return newline()
    const workspaceEdit = new vscode.WorkspaceEdit()
    workspaceEdit.set(document.uri, edits.map(withTabStops))
    await vscode.workspace.applyEdit(workspaceEdit)
  } finally {
    for (const listener of listeners) listener.dispose()
    cancellation.dispose()
  }
}

/**
 * The edit as the editor should apply it: an edit whose text carries a tab stop is a snippet, so
 * the caret lands where the site expects it rather than on the literal `$0`.
 */
function withTabStops(text: vscode.TextEdit): vscode.TextEdit | vscode.SnippetTextEdit {
  return PLACEHOLDER.test(text.newText)
    ? new vscode.SnippetTextEdit(text.range, new vscode.SnippetString(text.newText))
    : text
}

const PLACEHOLDER = /\$\d+|\{\d+:[^}]*\}/

/** The newline VS Code inserts when no site answers for the key. */
async function newline(): Promise<void> {
  await vscode.commands.executeCommand('type', { text: '\n' })
}
