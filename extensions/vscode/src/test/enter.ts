import * as assert from 'node:assert/strict'
import * as vscode from 'vscode'
import {
  blockRequest,
  check,
  type EditorSite,
  type HeldRequest,
  openMarked,
  replace,
  withSite,
} from './site.ts'

async function heldEnter(
  site: EditorSite,
  assertion: (
    document: vscode.TextDocument,
    entering: PromiseLike<unknown>,
    held: HeldRequest,
  ) => Promise<void>,
): Promise<void> {
  const held = await blockRequest(site, 'tola/onEnter')
  try {
    const { document, position } = await openMarked(site.source, '/// Original|\n')
    const editor = await vscode.window.showTextDocument(document)
    editor.selection = new vscode.Selection(position, position)
    const entering = vscode.commands.executeCommand('tola.onEnter')
    await held.reached
    await assertion(document, entering, held)
  } finally {
    await held.close()
  }
}

async function completed(entering: PromiseLike<unknown>): Promise<void> {
  let deadline: ReturnType<typeof setTimeout> | undefined
  try {
    await Promise.race([
      entering,
      new Promise<never>((_, reject) => {
        deadline = setTimeout(() => reject(new Error('Enter stayed blocked')), 5_000)
      }),
    ])
  } finally {
    clearTimeout(deadline)
  }
}

export async function runEnterChecks(): Promise<void> {
  await check('Enter follows source syntax', () =>
    withSite(async (site) => {
      const edits: [string, string][] = [
        ['- one|\n', '- one\n- \n'],
        ['+ one|\n', '+ one\n+ \n'],
        ['/// Doc|\n', '/// Doc\n/// \n'],
      ]
      for (const [marked, expected] of edits) {
        const { document, position } = await openMarked(site.source, marked)
        const editor = await vscode.window.showTextDocument(document)
        editor.selection = new vscode.Selection(position, position)
        await vscode.commands.executeCommand('tola.onEnter')
        assert.equal(document.getText(), expected)
      }
    }, { content: '' }))

  await check(
    'Delayed Enter continues comments',
    () =>
      withSite((site) =>
        heldEnter(site, async (document, entering, held) => {
          assert.equal(document.getText(), '/// Original\n')
          held.release()
          await completed(entering)
          assert.equal(document.getText(), '/// Original\n/// \n')
        })
      ),
  )

  await check(
    'Timed-out Enter inserts newline',
    () =>
      withSite((site) =>
        heldEnter(site, async (document, entering, held) => {
          await completed(entering)
          assert.equal(document.getText(), '/// Original\n\n')
          held.release()
          await vscode.commands.executeCommand('vscode.executeDocumentSymbolProvider', document.uri)
          assert.equal(document.getText(), '/// Original\n\n')
        })
      ),
  )

  await check('Consecutive Enter preserves input order', async () => {
    for (const release of [true, false]) {
      await withSite((site) =>
        heldEnter(site, async (document, entering, held) => {
          const following = vscode.commands.executeCommand('tola.onEnter')
          if (release) held.release()
          await completed(Promise.all([entering, following]))
          assert.equal(document.getText(), release ? '/// Original\n/// \n/// \n' : '/// Original\n\n\n')
        })
      )
    }
  })

  await check(
    'Delayed Enter preserves switched editors',
    () =>
      withSite((site) =>
        heldEnter(site, async (document, entering, held) => {
          const unrelated = await vscode.workspace.openTextDocument(site.plain)
          await vscode.window.showTextDocument(unrelated)
          await completed(entering)
          held.release()
          assert.equal(document.getText(), '/// Original\n')
          assert.equal(unrelated.getText(), 'Unrelated saved source.\n')
        })
      ),
  )

  await check(
    'Delayed Enter preserves changed buffers',
    () =>
      withSite((site) =>
        heldEnter(site, async (document, entering, held) => {
          await replace(document, 'Replaced while Enter was pending.\n')
          await completed(entering)
          held.release()
          assert.equal(document.getText(), 'Replaced while Enter was pending.\n')
        })
      ),
  )
}
