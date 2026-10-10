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
const entering = new WeakMap<vscode.TextEditor, Promise<boolean>>()

/**
 * Insert what the site's own syntax calls for at the cursor.
 *
 * A site that answers nothing keeps the editor's newline, so the key never does less than VS Code
 * would do on its own.
 */
export async function insertNewline(site: EnterSite | undefined): Promise<void> {
  const editor = vscode.window.activeTextEditor
  if (!editor) return
  // The next Enter reads the cursor after the previous insertion; an obsolete insertion ends
  // the queued keys as well, so they cannot edit a context the author has already left.
  const previous = entering.get(editor) ?? Promise.resolve(true)
  const insertion = previous.then((inserted) =>
    inserted && vscode.window.activeTextEditor === editor ? insert(editor, site) : false
  )
  entering.set(editor, insertion)
  try {
    await insertion
  } finally {
    if (entering.get(editor) === insertion) entering.delete(editor)
  }
}

async function insert(editor: vscode.TextEditor, site: EnterSite | undefined): Promise<boolean> {
  const document = editor.document
  if (!site) {
    await newline()
    return true
  }
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
  let deadline: ReturnType<typeof setTimeout> | undefined
  const expired = new Promise<undefined>((resolve) => {
    deadline = setTimeout(() => resolve(undefined), 1_000)
    listeners.push(cancellation.token.onCancellationRequested(() => resolve(undefined)))
  })
  try {
    if (cancellation.token.isCancellationRequested || !current()) return false
    const edits = await Promise.race([
      site.client
        .sendRequest(
          onEnterRequest,
          { uri: document.uri.toString(), position: selection.active },
          cancellation.token,
        )
        .then((reply) => site.client.protocol2CodeConverter.asTextEdits(reply?.edits))
        .catch(() => undefined),
      expired,
    ])
    if (cancellation.token.isCancellationRequested || !current()) return false
    if (!edits?.length) {
      await newline()
      return true
    }
    const workspaceEdit = new vscode.WorkspaceEdit()
    workspaceEdit.set(document.uri, edits.map(withTabStops))
    return await vscode.workspace.applyEdit(workspaceEdit)
  } finally {
    clearTimeout(deadline)
    cancellation.cancel()
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
