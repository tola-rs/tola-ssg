import * as assert from 'node:assert/strict'
import * as fs from 'node:fs/promises'
import * as path from 'node:path'
import * as vscode from 'vscode'
import { runGrammarChecks } from './grammar.ts'
import { runPreviewChecks } from './preview.ts'
import { runEnterChecks } from './enter.ts'
import { Assets } from '../assets.ts'
import { selectSite } from '../selection.ts'
import { SiteTasks } from '../tasks.ts'
import { Sites } from '../sites.ts'
import { SourceSynchronization } from '../synchronization.ts'
import { type SiteClient, VirtualSources } from '../virtual.ts'
import {
  blockRequest,
  blockServerStart,
  check,
  definitionTargets,
  disable,
  type EditorSite,
  type HeldStart,
  holdServerStart,
  openMarked,
  openPublishedPage,
  quoted,
  recordOutputChannels,
  recordServiceStarts,
  recordSiteSurfaces,
  replace,
  symbols,
  typstDocument,
  waitForClose,
  waitForDiagnostics,
  waitForOutput,
  withPreview,
  withSite,
  withSiteChoice,
  withSites,
  workbenchText,
} from './site.ts'

/** A definition inside `@tola/document` answers with a package document of the site that owns the source. */
const packageImport = '#import "@tola/document:0.0.0": current-document as identity\n#let value = ident|ity\n'

function fileTransfer(name: string, data: () => Promise<Uint8Array>): vscode.DataTransfer {
  const transfer = new vscode.DataTransfer()
  const file = new vscode.DataTransferItem('')
  file.asFile = () => ({ id: name, name, data })
  transfer.set('files', file)
  return transfer
}

async function withProtocolTrace(action: () => Promise<void>): Promise<void> {
  const trace = vscode.workspace.getConfiguration('tola')
  const previous = trace.inspect<string>('trace.server')?.workspaceValue
  await trace.update('trace.server', 'verbose', vscode.ConfigurationTarget.Workspace)
  try {
    await action()
  } finally {
    await vscode.workspace.getConfiguration('tola').update(
      'trace.server',
      previous,
      vscode.ConfigurationTarget.Workspace,
    )
  }
}

async function withoutUnhandledRejections(action: () => Promise<void>): Promise<void> {
  const rejections: unknown[] = []
  const rejected = (reason: unknown) => rejections.push(reason)
  process.on('unhandledRejection', rejected)
  try {
    await action()
    await new Promise<void>((resolve) => setImmediate(resolve))
    assert.deepEqual(rejections, [], 'Cancelled initialization left unhandled rejections')
  } finally {
    process.off('unhandledRejection', rejected)
  }
}

async function withoutRetainedDiagnostics(action: () => Promise<void>): Promise<void> {
  const create = vscode.languages.createDiagnosticCollection
  const retained = new Set<vscode.DiagnosticCollection>()
  vscode.languages.createDiagnosticCollection = (name) => {
    const collection = create(name)
    if (!name?.startsWith('tola:')) return collection
    retained.add(collection)
    const dispose = collection.dispose.bind(collection)
    collection.dispose = () => {
      retained.delete(collection)
      dispose()
    }
    return collection
  }
  try {
    await action()
    assert.equal(retained.size, 0, 'Stopped language services retained diagnostic collections')
  } finally {
    vscode.languages.createDiagnosticCollection = create
    for (const collection of retained) collection.dispose()
  }
}

async function openCheckedSource(
  site: EditorSite,
): Promise<{ entry: vscode.TextDocument; document: vscode.TextDocument }> {
  const entry = await vscode.workspace.openTextDocument(site.program)
  await vscode.window.showTextDocument(site.source)
  await workbenchText({ status: '(?<!not )checked' })
  return { entry, document: vscode.window.activeTextEditor!.document }
}

/** Open one site's heading and return the editor's increase action for it, with its own document. */
async function increaseHeadingAction(
  site: EditorSite,
): Promise<{ document: vscode.TextDocument; position: vscode.Position; command: vscode.Command }> {
  const { document, position } = await openMarked(site.source, '== Head|ing\n')
  const actions = await vscode.commands.executeCommand<vscode.CodeAction[]>(
    'vscode.executeCodeActionProvider',
    document.uri,
    new vscode.Range(position, position),
  )
  const action = actions.find((action) =>
    action.kind?.value === vscode.CodeActionKind.RefactorRewrite.value &&
    action.title.startsWith('increase') && action.command
  )
  assert.ok(action?.command, 'The heading offers no increase action')
  return { document, position, command: action.command }
}

/** Open one site's package import and return the definition the editor answers with. */
async function packageDefinition(site: EditorSite): Promise<{
  document: vscode.TextDocument
  position: vscode.Position
  uri: vscode.Uri
}> {
  const { document, position } = await openMarked(site.source, packageImport)
  const [uri] = await definitionTargets(document, position)
  assert.ok(uri, 'The site answers no package document for its source')
  return { document, position, uri }
}

/** A marker that arrives after `milliseconds`, so a case can tell a pending request from a settled one. */
function timedOut(milliseconds: number): Promise<'timeout'> {
  return new Promise((resolve) => setTimeout(() => resolve('timeout'), milliseconds))
}

/** Settle `promise` before the deadline, or fail with what the case is proving did not happen. */
async function within<T>(promise: Promise<T>, milliseconds: number, message: string): Promise<T> {
  let deadline: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        deadline = setTimeout(() => reject(new Error(message)), milliseconds)
      }),
    ])
  } finally {
    clearTimeout(deadline)
  }
}

export async function run(): Promise<void> {
  const extension = vscode.extensions.getExtension('tola.tola')
  recordOutputChannels()
  const surfaces = recordSiteSurfaces()
  assert.ok(extension, 'Tola extension was not loaded')
  let packageSources: VirtualSources | undefined
  let assetDrops: Assets | undefined
  const registerDrop = vscode.languages.registerDocumentDropEditProvider
  vscode.languages.registerDocumentDropEditProvider = (selector, provider, metadata) => {
    assetDrops = provider as Assets
    return registerDrop(selector, provider, metadata)
  }
  const registerContent = vscode.workspace.registerTextDocumentContentProvider
  vscode.workspace.registerTextDocumentContentProvider = (scheme, provider) => {
    if (scheme === 'tola-package') packageSources = provider as VirtualSources
    return registerContent(scheme, provider)
  }
  try {
    await extension.activate()
  } finally {
    vscode.workspace.registerTextDocumentContentProvider = registerContent
    vscode.languages.registerDocumentDropEditProvider = registerDrop
  }

  await runGrammarChecks(extension.extensionPath)

  await check('Requests wait for transmitted source versions', async () => {
    const document = await vscode.workspace.openTextDocument({ language: 'typst', content: '#value' })
    const synchronization = new SourceSynchronization()
    const cancellation = new vscode.CancellationTokenSource()
    let release!: () => void
    const transport = new Promise<void>((resolve) => release = resolve)
    try {
      const opened = synchronization.open(document, () => transport)
      let answered = false
      const waiting = synchronization.wait(document, document.version, cancellation.token).then((sent) => {
        answered = true
        return sent
      })
      await Promise.resolve()
      assert.equal(answered, false)
      release()
      await opened
      assert.equal(await waiting, true)
      const changed = synchronization.change(document, document.version + 1, () => Promise.resolve())
      const closed = synchronization.close(document, () => Promise.resolve())
      await Promise.all([changed, closed])
      const reopened = synchronization.wait(document, document.version, cancellation.token)
      cancellation.cancel()
      assert.equal(await reopened, false)
    } finally {
      release()
      cancellation.dispose()
      synchronization.dispose()
    }
  })

  await check('Synchronization belongs to one language service', async () => {
    const document = await vscode.workspace.openTextDocument({ language: 'typst', content: '#value' })
    const previous = new SourceSynchronization()
    const current = new SourceSynchronization()
    const cancellation = new vscode.CancellationTokenSource()
    try {
      await previous.open(document, () => Promise.resolve())
      let answered = false
      const waiting = current.wait(document, document.version, cancellation.token).then((sent) => {
        answered = true
        return sent
      })
      await Promise.resolve()
      assert.equal(answered, false)
      current.dispose()
      assert.equal(await waiting, false)
    } finally {
      previous.dispose()
      current.dispose()
      cancellation.dispose()
    }
  })

  await check('Package requests retain their source version while starting', async () => {
    const cancellation = new vscode.CancellationTokenSource()
    let ready!: (session: SiteClient) => void
    const starting = new Promise<SiteClient>((resolve) => ready = resolve)
    const requests: string[] = []
    const document = {
      uri: vscode.Uri.parse('tola-package:/tola/document/0.0.0/lib.typ?site=file:///site'),
      version: 1,
      isClosed: false,
      getText: () => '#let value = 1',
    }
    const session: SiteClient = {
      key: 'file:///site',
      lifetime: cancellation.token,
      client: {
        sendRequest: (type: { method: string }) => {
          requests.push(type.method)
          return Promise.resolve(
            type.method === 'tola/source' ? { text: document.getText() } : { data: [0, 0, 1, 0, 0] },
          )
        },
      } as unknown as SiteClient['client'],
    }
    const virtual = new VirtualSources(() => session, () => starting)
    try {
      const pending = virtual.provideDocumentSemanticTokens(
        document as vscode.TextDocument,
        cancellation.token,
      )
      document.version += 1
      ready(session)
      await assert.rejects(pending, vscode.CancellationError)
      assert.deepEqual(requests, [])
      const current = await virtual.provideDocumentSemanticTokens(
        document as vscode.TextDocument,
        cancellation.token,
      )
      assert.deepEqual([...current!.data], [0, 0, 1, 0, 0])
    } finally {
      ready(session)
      virtual.dispose()
      cancellation.dispose()
    }
  })

  await check('Cancelled package requests stop waiting for services', async () => {
    if (process.platform === 'win32') return
    await withSite(async (site) => {
      assert.ok(packageSources, 'The package source provider is not registered')
      const blocked = await blockServerStart(site)
      const cancellation = new vscode.CancellationTokenSource()
      const uri = vscode.Uri.parse('tola-package:/tola/document/0.0.0/lib.typ').with({
        query: `site=${vscode.Uri.file(site.root)}`,
      })
      const document = { uri, version: 1, isClosed: false } as vscode.TextDocument
      try {
        const content = packageSources.provideTextDocumentContent(uri, cancellation.token)
        const tokens = packageSources.provideDocumentSemanticTokens(document, cancellation.token)
        const cancelled = Promise.all([
          assert.rejects(content, vscode.CancellationError),
          assert.rejects(tokens, vscode.CancellationError),
        ])
        cancellation.cancel()
        await within(cancelled, 1_000, 'Cancelled requests kept waiting for initialization')
      } finally {
        cancellation.dispose()
        await blocked.close()
      }
    })
  })

  await check('Closed untitled requests preserve cancellation', () =>
    withSite(async (site) => {
      const output = vscode.window.createOutputChannel('Tola closed source check')
      const connections = new Sites(output, () => Promise.resolve(), () => {})
      try {
        await connections.refresh()
        await vscode.window.showTextDocument(site.source)
        const owner = connections.enterSite()
        assert.ok(owner)
        const uri = vscode.Uri.file(path.join(site.root, 'closed-request.typ')).with({ scheme: 'untitled' })
        await vscode.commands.executeCommand('vscode.open', uri)
        const closed = waitForClose(uri)
        await vscode.commands.executeCommand('workbench.action.closeActiveEditor')
        await closed
        await assert.rejects(
          owner.client.sendRequest('tola/onEnter', {
            uri: uri.toString(),
            position: { line: 0, character: 0 },
          }),
          vscode.CancellationError,
        )
      } finally {
        await connections.close()
        output.dispose()
      }
    }))

  await check('Edited sources cancel page navigation quietly', () =>
    withSite(async (site) => {
      const held = await blockRequest(site, 'tola/route')
      const document = await vscode.workspace.openTextDocument(site.source)
      await vscode.window.showTextDocument(document)
      const errors: string[] = []
      const show = vscode.window.showErrorMessage
      vscode.window.showErrorMessage = ((message: string, ...args: unknown[]) => {
        errors.push(message)
        return Reflect.apply(show, vscode.window, [message, ...args])
      }) as typeof show
      try {
        const opening = vscode.commands.executeCommand('tola.openPage', site.source)
        await within(held.reached, 5_000, 'Open Page never asked for the source routes')
        await replace(document, 'Changed while page navigation was pending.\n')
        held.release()
        await within(Promise.resolve(opening), 5_000, 'Cancelled page navigation did not settle')
        assert.deepEqual(errors, [])
      } finally {
        vscode.window.showErrorMessage = show
        await held.close()
      }
    }))

  await check('Documents folders answer language requests', () =>
    withSite(async (workspace) => {
      const { document, position } = await openMarked(workspace.source, '#let value = 42\n#va|lue\n')
      const hover = await vscode.commands.executeCommand<vscode.Hover[]>(
        'vscode.executeHoverProvider',
        document.uri,
        position,
      )
      assert.match(
        hover.flatMap((answer) =>
          answer.contents.map((content) => typeof content === 'string' ? content : content.value)
        ).join('\n'),
        /let value = int;/,
      )
      const broken = vscode.Uri.file(path.join(workspace.root, 'broken.typ'))
      await fs.writeFile(broken.fsPath, '#missing_document_value\n')
      await vscode.workspace.openTextDocument(broken)
      await waitForDiagnostics(broken, (diagnostics) =>
        diagnostics.some((diagnostic) =>
          diagnostic.message.includes('missing_document_value')
        ))
      assert.deepEqual(vscode.languages.getDiagnostics(document.uri), [])
    }, { documents: true, program: '#missing_site_program\n' }))

  await check(
    'Documents folders offer no site features',
    () =>
      withPreview((opened) =>
        withSite(async (workspace) => {
          await vscode.window.showTextDocument(workspace.source)
          assert.equal(surfaces.siteSelected(), false)
          assert.deepEqual(await surfaces.pages(), [])
          assert.deepEqual(await vscode.tasks.fetchTasks({ type: 'tola' }), [])
          const lenses = await vscode.commands.executeCommand<vscode.CodeLens[]>(
            'vscode.executeCodeLensProvider',
            workspace.source,
          )
          assert.deepEqual(lenses ?? [], [])
          await vscode.commands.executeCommand('tola.openPreview', workspace.source)
          assert.match(
            await workbenchText({ contains: 'no site answers', dismiss: true }),
            /no site answers/,
          )
          assert.deepEqual(opened, [])
        }, { documents: true })
      ),
  )

  await check(
    'Created configuration selects site features',
    () =>
      withPreview((opened) =>
        withSite(async (workspace) => {
          await vscode.window.showTextDocument(workspace.source)
          assert.equal(surfaces.siteSelected(), false)
          await fs.writeFile(
            workspace.configuration.fsPath,
            '[site]\ntitle = "Created site"\n[server]\nport = 0\n',
          )
          await workbenchText({ status: 'Start Preview', capture: true })
          assert.equal(surfaces.siteSelected(), true)
          const [task, ...others] = await vscode.tasks.fetchTasks({ type: 'tola' })
          assert.ok(task && !others.length)
          assert.ok(task.execution instanceof vscode.ProcessExecution)
          assert.deepEqual(task.execution.args.slice(0, 3), [
            'build',
            '--config',
            workspace.configuration.fsPath,
          ])
          await openPublishedPage('tola.openPreview', workspace.source, opened)
          assert.equal((await surfaces.pages())[0]?.label, '/preview/')
        }, { documents: true })
      ),
  )

  await check('Delayed actions refuse changed sources', () =>
    withSite(async (site) => {
      const { document, command } = await increaseHeadingAction(site)
      await replace(document, 'Changed before action application.\n')
      assert.equal(
        await vscode.commands.executeCommand(command.command, ...(command.arguments ?? [])),
        false,
      )
      const notification = await workbenchText({ error: true, dismiss: true })
      assert.match(notification, /no longer current/)
      assert.doesNotMatch(notification, /Restart/)
      assert.equal(document.getText(), 'Changed before action application.\n')
    }))

  await check('Delayed actions refuse restarted services', () =>
    withSite(async (site) => {
      const { document, command } = await increaseHeadingAction(site)
      await vscode.commands.executeCommand('tola.restart')
      assert.equal(
        await vscode.commands.executeCommand(command.command, ...(command.arguments ?? [])),
        false,
      )
      assert.match(await workbenchText({ error: true, dismiss: true }), /no longer current/)
      assert.equal(document.getText(), '== Heading\n')
    }))

  await check(
    'Heading actions change rendered depth',
    () =>
      withPreview((opened) =>
        withSite(async (site) => {
          const { document, position, command } = await increaseHeadingAction(site)
          const editor = await vscode.window.showTextDocument(document)
          editor.selection = new vscode.Selection(position, position)
          assert.equal(await document.save(), true)
          await vscode.commands.executeCommand('tola.openPreview', site.source)
          const before = opened[0]!.html.match(/<h(\d)\b[^>]*>Heading<\/h\d>/)
          assert.ok(before)
          assert.equal(
            await vscode.commands.executeCommand(command.command, ...(command.arguments ?? [])),
            true,
          )
          assert.equal(document.getText(), '=== Heading\n')
          assert.equal(await document.save(), true)
          await vscode.commands.executeCommand('tola.openPreview', site.source)
          const after = opened[1]!.html.match(/<h(\d)\b[^>]*>Heading<\/h\d>/)
          assert.ok(after)
          assert.equal(Number(after[1]), Number(before[1]) + 1)
        })
      ),
  )

  await check(
    'Restarting stopped services preserves other folders',
    () =>
      withSites(2, async ([site, other]) => {
        assert.ok(site && other)
        const available = waitForOutput(`Started Tola language service for ${other.name}.`)
        const healthy = await recordServiceStarts(other)
        await available
        assert.equal((await symbols(other.source)).length, 1)
        const healthyStarts = await healthy.starts('lsp')
        const recorded = waitForOutput('Started Tola language service for')
        const service = await recordServiceStarts(site)
        // The recorded service is the current one once its own client finished initializing.
        await recorded
        assert.equal((await symbols(site.source)).length, 1)
        const pid = await service.servicePid('lsp')
        assert.ok(pid, 'The recorded service never reported its own process')
        process.kill(pid, 'SIGKILL')
        // The button asks for a restart; the client answers once its service is up again.
        const restarted = waitForOutput('Started Tola language service for')
        await workbenchText({ button: 'Restart' })
        await restarted
        assert.equal((await symbols(site.source)).length, 1)
        assert.equal(await healthy.starts('lsp'), healthyStarts)
      }, { content: '= Restored heading\n' }),
  )

  await check('Restart cancels pending initialization', async () => {
    if (process.platform === 'win32') return
    await withoutUnhandledRejections(() =>
      withSite(async (site) => {
        const blocked = await blockServerStart(site)
        try {
          await vscode.window.showTextDocument(site.source)
          await workbenchText({ status: 'service starting', capture: true })
          await vscode.workspace.getConfiguration('tola', site.source)
            .update('serverPath', process.env.TOLA_TEST_BINARY, vscode.ConfigurationTarget.WorkspaceFolder)
          await vscode.commands.executeCommand('tola.restart')
          assert.equal((await symbols(site.source)).length, 1)
        } finally {
          await blocked.dispose()
        }
      }, { content: '= Restored heading\n' })
    )
  })

  await check('Closing cancels pending initialization', async () => {
    if (process.platform === 'win32') return
    await withoutUnhandledRejections(() =>
      withSite(async (site) => {
        const blocked = await blockServerStart(site)
        const output = vscode.window.createOutputChannel('Tola shutdown check')
        let started!: () => void
        const reached = new Promise<void>((resolve) => started = resolve)
        const channel = new Proxy(output, {
          get(target, property) {
            const value = Reflect.get(target, property)
            if (property === 'appendLine') {
              return (message: string) => {
                target.appendLine(message)
                if (message.startsWith('Starting language services')) started()
              }
            }
            return typeof value === 'function' ? value.bind(target) : value
          },
        })
        const connections = new Sites(channel, () => Promise.resolve(), () => {})
        try {
          const refreshing = connections.refresh()
          await reached
          await connections.close()
          await refreshing
        } finally {
          await connections.close()
          output.dispose()
          await vscode.workspace.getConfiguration('tola', site.source)
            .update('serverPath', process.env.TOLA_TEST_BINARY, vscode.ConfigurationTarget.WorkspaceFolder)
          await vscode.commands.executeCommand('tola.restart')
          await blocked.dispose()
        }
      })
    )
  })

  await check('Pending initialization leaves other folders available', async () => {
    if (process.platform === 'win32') return
    await withoutUnhandledRejections(() =>
      withSites(2, async ([pending, available]) => {
        assert.ok(pending && available)
        const blocked = await blockServerStart(pending)
        const output = vscode.window.createOutputChannel('Tola folder startup check')
        const connections = new Sites(output, () => Promise.resolve(), () => {})
        let refreshing: Promise<void> | undefined
        try {
          const started = waitForOutput(`Started Tola language service for ${available.name}.`)
          refreshing = connections.refresh()
          await started
          assert.equal(connections.statusSite(available.source)?.state, 'ready')
          assert.equal(connections.statusSite(pending.source)?.state, 'starting')
        } finally {
          await connections.close()
          await refreshing
          output.dispose()
          await vscode.workspace.getConfiguration('tola', pending.source)
            .update('serverPath', process.env.TOLA_TEST_BINARY, vscode.ConfigurationTarget.WorkspaceFolder)
          await vscode.commands.executeCommand('tola.restart')
          await blocked.dispose()
        }
      })
    )
  })

  await check(
    'Missing services report startup failure',
    () =>
      withoutRetainedDiagnostics(() =>
        withoutUnhandledRejections(() =>
          withSite(async (site) => {
            const settings = vscode.workspace.getConfiguration('tola', site.source)
            try {
              await settings.update(
                'serverPath',
                path.join(site.root, 'missing-tola'),
                vscode.ConfigurationTarget.WorkspaceFolder,
              )
              await vscode.commands.executeCommand('tola.restart')
              assert.match(
                await workbenchText({ contains: 'Tola could not start', dismiss: true }),
                /Tola could not start/,
              )
            } finally {
              await settings.update(
                'serverPath',
                process.env.TOLA_TEST_BINARY,
                vscode.ConfigurationTarget.WorkspaceFolder,
              )
              await vscode.commands.executeCommand('tola.restart')
            }
          })
        )
      ),
  )

  await check('Delayed transfers discard switched editors', () =>
    withSites(2, async ([first, second]) => {
      assert.ok(first && second)
      const document = await vscode.workspace.openTextDocument(first.source)
      await vscode.window.showTextDocument(document)
      const copied = await fs.readFile(first.plain.fsPath)
      let release!: () => void
      const held = new Promise<Uint8Array>((resolve) => {
        release = () => resolve(copied)
      })
      let observed!: () => void
      const reached = new Promise<void>((resolve) => {
        observed = resolve
      })
      const transfer = new vscode.DataTransfer()
      const transferred = new vscode.DataTransferItem('')
      transferred.asFile = () => ({
        id: 'delayed-transfer',
        name: 'transferred.txt',
        data: () => {
          observed()
          return held
        },
      })
      transfer.set('files', transferred)
      const provider = new Assets(
        (source) => ({
          root: source === document ? first.root : second.root,
          directory: 'assets',
          entry: source === document ? first.program : second.program,
          sources: { files: [], trees: [] },
          generated: [],
          current: () => true,
        }),
        (error) => {
          throw error
        },
      )
      const cancellation = new vscode.CancellationTokenSource()
      try {
        const dropping = provider.provideDocumentDropEdits(
          document,
          new vscode.Position(0, 0),
          transfer,
          cancellation.token,
        )
        await reached
        await vscode.window.showTextDocument(second.source)
        release()
        assert.equal(await dropping, undefined)
        await assert.rejects(fs.stat(path.join(first.root, 'assets/transferred.txt')), { code: 'ENOENT' })
        await assert.rejects(fs.stat(path.join(second.root, 'assets/transferred.txt')), { code: 'ENOENT' })
      } finally {
        release()
        cancellation.dispose()
      }
    }))

  await check('Dropped assets publish through the Bundle entry', () =>
    withProtocolTrace(async () => {
      for (
        const [target, custom, publisher] of [
          ['entry', false, 'bundle'],
          ['content', true, 'bundle'],
          ['content', true, 'tree'],
          ['content', false, 'file'],
        ] as const
      ) {
        let entryUri: vscode.Uri
        const name = '中文 cost$1"}\\.bin'
        const copiedName = '中文 cost$1-}--1.bin'
        const bytes = new Uint8Array([0, 255, 128, 65, 10])
        await withSite(async (site) => {
          assert.ok(assetDrops)
          const preparedEntry = waitForOutput((line) =>
            line.includes('"entry"') && line.includes(entryUri!.path.split('/').pop()!)
          )
          const entry = await vscode.workspace.openTextDocument(entryUri!)
          if (custom) {
            await replace(entry, entry.getText().replace('preview/index.html', 'authored/index.html'))
          }
          await preparedEntry
          const document = target === 'entry' ? entry : await vscode.workspace.openTextDocument(site.source)
          await vscode.window.showTextDocument(document)
          await workbenchText({ status: '(?<!not )checked|failed' })
          const entryBefore = entry.getText()
          const contentBefore = document.getText()
          const transfer = fileTransfer(name, () => Promise.resolve(bytes))
          const cancellation = new vscode.CancellationTokenSource()
          try {
            const [drop] = await assetDrops.provideDocumentDropEdits(
              document,
              new vscode.Position(0, 0),
              transfer,
              cancellation.token,
            ) ?? []
            assert.ok(drop?.additionalEdit, `${target}/${publisher}: no publishing edit`)
            assert.equal(drop.insertText, '')
            // Apply the offered WorkspaceEdit in the real host; this does not simulate a native drop gesture.
            assert.equal(await vscode.workspace.applyEdit(drop.additionalEdit), true)
            if (target === 'content') assert.equal(document.getText(), contentBefore)
            if (publisher === 'bundle') {
              assert.ok(
                drop.additionalEdit.entries().some(([uri]) => uri.toString() === entry.uri.toString()),
              )
              assert.ok(entry.getText().includes('#asset('))
              assert.ok(entry.getText().endsWith(entryBefore.replace(/^\uFEFF/, '')))
            } else assert.equal(entry.getText(), entryBefore)
            if (custom) assert.equal(entry.getText().replaceAll('\r\n', '').includes('\n'), false)
            assert.deepEqual(
              new Uint8Array(await fs.readFile(path.join(site.root, 'assets', copiedName))),
              bytes,
            )
            assert.equal(await entry.save(), true)
            await vscode.commands.executeCommand('tola.build')
            assert.deepEqual(
              new Uint8Array(await fs.readFile(path.join(site.root, 'public/assets', copiedName))),
              bytes,
            )
            if (custom) {
              assert.ok((await fs.stat(path.join(site.root, 'public/authored/index.html'))).isFile())
            }
          } finally {
            cancellation.dispose()
          }
        }, {
          prepare: async (site) => {
            const directory = path.join(site.root, 'assets')
            await fs.mkdir(directory)
            await fs.writeFile(path.join(directory, '中文 cost$1-}-.bin'), 'existing asset')
            entryUri = site.program
            if (custom) {
              const entry = path.join(site.root, 'bundle/custom.typ')
              await fs.mkdir(path.dirname(entry))
              await fs.writeFile(
                entry,
                '\uFEFF#document("preview/index.html", format:"html")[#include "../content/page.typ"]\r\n',
              )
              await fs.appendFile(site.configuration.fsPath, '\n[build]\nentry="bundle/custom.typ"\n')
              if (process.platform !== 'win32') {
                const alias = path.join(site.root, 'entry-alias.typ')
                await fs.symlink('bundle/custom.typ', alias)
                entryUri = vscode.Uri.file(alias)
              } else entryUri = vscode.Uri.file(entry)
            }
            if (publisher === 'tree') {
              await fs.appendFile(
                site.configuration.fsPath,
                '\n[assets]\ntrees=[{source="assets",url-prefix="/assets"}]\n',
              )
            }
            if (publisher === 'file') {
              await fs.appendFile(
                site.configuration.fsPath,
                `\n[assets]\nfiles=[{source=${JSON.stringify('assets/' + copiedName)},url=${
                  JSON.stringify('/assets/' + copiedName)
                }}]\n`,
              )
            }
          },
        })
      }
    }))

  await check(
    'Open entry aliases receive one publishing edit each',
    () =>
      withProtocolTrace(() =>
        withSite(async (site) => {
          if (process.platform === 'win32') return
          assert.ok(assetDrops)
          const alias = vscode.Uri.file(path.join(site.root, 'entry-alias.typ'))
          const entryPacket = waitForOutput((line) =>
            line.includes('"entry"') && line.includes('entry-alias.typ')
          )
          const entry = await vscode.workspace.openTextDocument(site.program)
          const linked = await vscode.workspace.openTextDocument(alias)
          assert.notEqual(entry.uri.toString(), linked.uri.toString())
          assert.equal(entry.getText(), linked.getText())
          await vscode.window.showTextDocument(site.source)
          await entryPacket
          const document = vscode.window.activeTextEditor!.document
          const cancellation = new vscode.CancellationTokenSource()
          try {
            const [drop] = await assetDrops.provideDocumentDropEdits(
              document,
              new vscode.Position(0, 0),
              fileTransfer('aliases.bin', () => Promise.resolve(new Uint8Array([255]))),
              cancellation.token,
            ) ?? []
            assert.ok(drop?.additionalEdit)
            assert.equal(await vscode.workspace.applyEdit(drop.additionalEdit), true)
            assert.equal(entry.getText(), linked.getText())
            assert.ok(entry.getText().includes('#asset('))
            await entry.save()
            await vscode.commands.executeCommand('tola.build')
            assert.deepEqual(
              new Uint8Array(await fs.readFile(path.join(site.root, 'public/assets/aliases.bin'))),
              new Uint8Array([255]),
            )
            await replace(linked, linked.getText() + '\nDifferent entry text\n')
            assert.equal(
              await assetDrops.provideDocumentDropEdits(
                document,
                new vscode.Position(0, 0),
                fileTransfer('conflict.bin', () => Promise.resolve(new Uint8Array([255]))),
                cancellation.token,
              ),
              undefined,
            )
            assert.match(
              await workbenchText({ error: true, dismiss: true }),
              /entry.*(copies|text)|conflicting/i,
            )
            await assert.rejects(fs.stat(path.join(site.root, 'assets/conflict.bin')), { code: 'ENOENT' })
            const configuredPacket = waitForOutput((line) =>
              line.includes('"trees"') && line.includes('/assets"')
            )
            await fs.appendFile(
              site.configuration.fsPath,
              '\n[assets]\ntrees=[{source="assets",url-prefix="/assets"}]\n',
            )
            await configuredPacket
            const [configured] = await assetDrops.provideDocumentDropEdits(
              document,
              new vscode.Position(0, 0),
              fileTransfer('configured.bin', () => Promise.resolve(new Uint8Array([255]))),
              cancellation.token,
            ) ?? []
            assert.ok(configured?.additionalEdit)
            assert.equal(configured.additionalEdit.entries().length, 0)
            assert.equal(await vscode.workspace.applyEdit(configured.additionalEdit), true)
            assert.deepEqual(
              new Uint8Array(await fs.readFile(path.join(site.root, 'assets/configured.bin'))),
              new Uint8Array([255]),
            )
          } finally {
            cancellation.dispose()
          }
        }, {
          prepare: async (site) => {
            if (process.platform !== 'win32') {
              await fs.symlink('site.typ', path.join(site.root, 'entry-alias.typ'))
            }
          },
        })
      ),
  )

  await check('asset names stay publishable', () =>
    withSite(async (site) => {
      assert.ok(assetDrops)
      const { entry, document } = await openCheckedSource(site)
      const bytes = new Uint8Array([0, 255, 128])
      const names = [
        ['中文 $}.bin', '中文 $}.bin'],
        ['CONIN$.bin', '_CONIN$.bin'],
        ['lpt¹.txt', '_lpt¹.txt'],
        ['ending. ', 'ending-'],
        ['many.bin', 'many-101.bin'],
        ['界'.repeat(85) + '.bin', '界'.repeat(83) + '-1.bin'],
      ] as const
      const cancellation = new vscode.CancellationTokenSource()
      try {
        for (const [name] of names) {
          const [drop]: vscode.DocumentDropEdit[] = await assetDrops.provideDocumentDropEdits(
            document,
            new vscode.Position(0, 0),
            fileTransfer(name, () => Promise.resolve(bytes)),
            cancellation.token,
          ) ?? []
          assert.ok(drop?.additionalEdit, name)
          assert.equal(await vscode.workspace.applyEdit(drop.additionalEdit), true)
        }
        await entry.save()
        await vscode.commands.executeCommand('tola.build')
        for (const [, expected] of names) {
          assert.deepEqual(
            new Uint8Array(await fs.readFile(path.join(site.root, 'public/静态 文件', expected))),
            bytes,
          )
        }
      } finally {
        cancellation.dispose()
      }
    }, {
      prepare: async (site) => {
        await fs.mkdir(path.join(site.root, '静态 文件'))
        await Promise.all(Array.from({ length: 101 }, (_, index) =>
          fs.writeFile(
            path.join(site.root, '静态 文件', `many${index ? `-${index}` : ''}.bin`),
            'existing asset',
          )))
        await fs.writeFile(path.join(site.root, '静态 文件', '界'.repeat(83) + '.bin'), 'existing asset')
        const settings = path.join(site.root, '.vscode/settings.json')
        const configured = JSON.parse(await fs.readFile(settings, 'utf8'))
        configured['tola.assetDirectory'] = '静态 文件'
        await fs.writeFile(settings, JSON.stringify(configured))
      },
    }))

  await check('asset dirs stay inside inputs', () =>
    withSite(async (site) => {
      assert.ok(assetDrops)
      const { entry, document } = await openCheckedSource(site)
      const before = entry.getText()
      const cancellation = new vscode.CancellationTokenSource()
      try {
        for (const directory of ['../outside-assets', 'bad"directory', '_tola/assets', 'release']) {
          await vscode.workspace.getConfiguration('tola', document.uri)
            .update('assetDirectory', directory, vscode.ConfigurationTarget.WorkspaceFolder)
          const dropped = await assetDrops.provideDocumentDropEdits(
            document,
            new vscode.Position(0, 0),
            fileTransfer('asset.bin', () => Promise.resolve(new Uint8Array([255]))),
            cancellation.token,
          )
          assert.equal(dropped, undefined)
          assert.match(await workbenchText({ error: true, dismiss: true }), /tola\.assetDirectory/)
        }
        assert.equal(entry.getText(), before)
        await assert.rejects(fs.stat(path.join(path.dirname(site.root), 'outside-assets')), {
          code: 'ENOENT',
        })
        await assert.rejects(fs.stat(path.join(site.root, 'release/asset.bin')), { code: 'ENOENT' })
      } finally {
        cancellation.dispose()
      }
    }, { configuration: '[build]\npublish-dir="release"\n' }))

  await check('new asset dirs join the edit', () =>
    withSite(async (site) => {
      assert.ok(assetDrops)
      const { entry, document } = await openCheckedSource(site)
      await vscode.workspace.getConfiguration('tola', document.uri)
        .update('assetDirectory', 'new assets/nested', vscode.ConfigurationTarget.WorkspaceFolder)
      const cancellation = new vscode.CancellationTokenSource()
      try {
        const [drop] = await assetDrops.provideDocumentDropEdits(
          document,
          new vscode.Position(0, 0),
          fileTransfer('new.bin', () => Promise.resolve(new Uint8Array([255]))),
          cancellation.token,
        ) ?? []
        assert.ok(drop?.additionalEdit)
        await assert.rejects(fs.stat(path.join(site.root, 'new assets')), { code: 'ENOENT' })
        assert.equal(await vscode.workspace.applyEdit(drop.additionalEdit), true)
        await entry.save()
        await vscode.commands.executeCommand('tola.build')
        assert.deepEqual(
          new Uint8Array(await fs.readFile(path.join(site.root, 'public/new assets/nested/new.bin'))),
          new Uint8Array([255]),
        )
      } finally {
        cancellation.dispose()
      }
    }))

  await check('Declared external asset trees publish copied bytes', async () => {
    if (process.platform === 'win32') return
    for (const [declared, pure] of [[false, false], [true, false], [true, true]] as const) {
      let shared: string
      await withSite(async (site) => {
        assert.ok(assetDrops)
        const entry = await vscode.workspace.openTextDocument(site.program)
        await vscode.window.showTextDocument(site.source)
        await workbenchText({ status: '(?<!not )checked|failed' })
        const document = vscode.window.activeTextEditor!.document
        const before = entry.getText()
        const bytes = new Uint8Array([0, 255, 128])
        const cancellation = new vscode.CancellationTokenSource()
        try {
          const edits = await assetDrops.provideDocumentDropEdits(
            document,
            new vscode.Position(0, 0),
            fileTransfer('external.bin', () => Promise.resolve(bytes)),
            cancellation.token,
          )
          if (!declared || pure) {
            assert.equal(edits, undefined)
            if (!pure) {
              assert.match(await workbenchText({ error: true, dismiss: true }), /tola\.assetDirectory/)
            }
            await assert.rejects(fs.stat(path.join(shared!, 'external.bin')), { code: 'ENOENT' })
          } else {
            const [drop] = edits ?? []
            assert.ok(drop?.additionalEdit)
            assert.equal(await vscode.workspace.applyEdit(drop.additionalEdit), true)
            assert.equal(entry.getText(), before)
            await vscode.commands.executeCommand('tola.build')
            assert.deepEqual(new Uint8Array(await fs.readFile(path.join(shared!, 'external.bin'))), bytes)
            assert.deepEqual(
              new Uint8Array(await fs.readFile(path.join(site.root, 'public/assets/external.bin'))),
              bytes,
            )
          }
        } finally {
          cancellation.dispose()
        }
      }, {
        prepare: async (site) => {
          shared = path.join(path.dirname(site.root), 'shared-assets')
          await fs.mkdir(shared)
          await fs.symlink(shared, path.join(site.root, 'assets'))
          if (declared) {
            await fs.appendFile(
              site.configuration.fsPath,
              '\n[assets]\ntrees=[{source="assets",url-prefix="/assets"}]\n',
            )
          }
          if (pure) {
            const runner = path.join(site.root, 'pure-assets.sh')
            await fs.writeFile(
              runner,
              `#!/bin/sh\nexec ${quoted(process.env.TOLA_TEST_BINARY!)} "$@" --pure\n`,
              { mode: 0o755 },
            )
            const settingsPath = path.join(site.root, '.vscode/settings.json')
            const settings = JSON.parse(await fs.readFile(settingsPath, 'utf8'))
            settings['tola.serverPath'] = runner
            await fs.writeFile(settingsPath, JSON.stringify(settings))
          }
        },
      })
    }
  })

  await check('invalid config offers no edits', () =>
    withSite(async (site) => {
      assert.ok(assetDrops)
      const { entry, document } = await openCheckedSource(site)
      const original = entry.getText()
      await fs.writeFile(site.configuration.fsPath, '[site\n')
      await waitForDiagnostics(site.configuration, (reports) =>
        reports.some((report) => report.severity === vscode.DiagnosticSeverity.Error))
      const cancellation = new vscode.CancellationTokenSource()
      try {
        assert.equal(
          await assetDrops.provideDocumentDropEdits(
            document,
            new vscode.Position(0, 0),
            fileTransfer('invalid.bin', () =>
              Promise.resolve(new Uint8Array([255]))),
            cancellation.token,
          ),
          undefined,
        )
        assert.equal(entry.getText(), original)
      } finally {
        cancellation.dispose()
      }
    }))

  await check('Changed entry buffers cancel pending assets', () =>
    withSite(async (site) => {
      assert.ok(assetDrops)
      const { entry, document } = await openCheckedSource(site)
      let release!: (bytes: Uint8Array) => void
      let observe!: () => void
      const held = new Promise<Uint8Array>((resolve) => {
        release = resolve
      })
      const reached = new Promise<void>((resolve) => {
        observe = resolve
      })
      const cancellation = new vscode.CancellationTokenSource()
      try {
        const dropping = assetDrops.provideDocumentDropEdits(
          document,
          new vscode.Position(0, 0),
          fileTransfer('waiting.bin', () => {
            observe()
            return held
          }),
          cancellation.token,
        )
        await reached
        await replace(entry, '#document("changed.html",format:"html")[Author edit]\n')
        release(new Uint8Array([255]))
        assert.equal(await dropping, undefined)
        assert.equal(entry.getText(), '#document("changed.html",format:"html")[Author edit]\n')
        await assert.rejects(fs.stat(path.join(site.root, 'assets/waiting.bin')), { code: 'ENOENT' })
      } finally {
        release(new Uint8Array())
        cancellation.dispose()
      }
    }))

  await check('unconfirmed entry offers no edits', () =>
    withSite(async (site) => {
      assert.ok(assetDrops)
      const entry = await vscode.workspace.openTextDocument(site.program)
      const config = await vscode.workspace.openTextDocument(site.configuration)
      await fs.writeFile(path.join(site.root, 'next.typ'), '#document("next.html",format:"html")[Next]\n')
      const held = await blockRequest(site, 'textDocument/didChange')
      const cancellation = new vscode.CancellationTokenSource()
      try {
        await vscode.window.showTextDocument(site.source)
        await workbenchText({ status: '(?<!not )checked' })
        const document = vscode.window.activeTextEditor!.document
        const original = entry.getText()
        await replace(config, config.getText() + '\n[build]\nentry="next.typ"\n')
        await within(held.reached, 10_000, 'Config change did not reach the service gate')
        const transfer = fileTransfer('configuration.bin', () => Promise.resolve(new Uint8Array([255])))
        assert.equal(
          await assetDrops.provideDocumentDropEdits(
            document,
            new vscode.Position(0, 0),
            transfer,
            cancellation.token,
          ),
          undefined,
        )
        assert.equal(await config.save(), true)
        assert.equal(
          await assetDrops.provideDocumentDropEdits(
            document,
            new vscode.Position(0, 0),
            transfer,
            cancellation.token,
          ),
          undefined,
        )
        assert.equal(entry.getText(), original)
      } finally {
        held.release()
        await held.close()
        cancellation.dispose()
      }
    }))

  await check('Folder tasks build their own site', () =>
    withSites(2, async ([first, second]) => {
      assert.ok(first && second)
      const folders = [first, second].map((site) => vscode.workspace.getWorkspaceFolder(site.source)!)
      const selections = await Promise.all(folders.map((folder) =>
        selectSite(folder, vscode.Uri.file(path.join(folder.uri.fsPath, 'selected config.toml')))
      ))
      assert.ok(selections[0] && selections[1])
      const provider = new SiteTasks(() => [selections[0]!, selections[1]!])
      const task = provider.resolveTask(
        new vscode.Task(
          { type: 'tola', verb: 'build' },
          folders[1]!,
          'Scoped build',
          'tola',
        ),
      )
      assert.ok(task)
      const ended = new Promise<void>((resolve, reject) => {
        const deadline = setTimeout(
          () =>
            reject(new Error('The scoped task never finished')),
          60_000,
        )
        const listener = vscode.tasks.onDidEndTaskProcess((event) => {
          if (event.execution.task.name !== task.name) {
            return
          }
          clearTimeout(deadline)
          listener.dispose()
          if (event.exitCode === 0) resolve()
          else reject(new Error(`The scoped task exited ${event.exitCode}`))
        })
      })
      await vscode.tasks.executeTask(task)
      await ended
      assert.match(
        await fs.readFile(path.join(second.root, 'task-output/preview/index.html'), 'utf8'),
        /Published editor page/,
      )
      await assert.rejects(fs.stat(path.join(first.root, 'task-output')), { code: 'ENOENT' })
    }, { configuration: '[site]\ntitle = "Scoped task"\n[build]\npublish-dir = "task-output"\n' }))

  await check('Workspace tasks refuse ambiguous sites', () =>
    withSites(2, async (sites) => {
      const selections = await Promise.all(sites.map((site) => {
        const folder = vscode.workspace.getWorkspaceFolder(site.source)!
        return selectSite(folder, vscode.Uri.file(path.join(folder.uri.fsPath, 'selected config.toml')))
      }))
      const provider = new SiteTasks(() => selections.filter((selected) => selected !== undefined))
      assert.equal(
        provider.resolveTask(
          new vscode.Task(
            { type: 'tola', verb: 'build' },
            vscode.TaskScope.Workspace,
            'Ambiguous build',
            'tola',
          ),
        ),
        undefined,
      )
    }))

  await check('Folder tasks resolve duplicate names by scope', () =>
    withSites(2, async (sites) => {
      const selections = await Promise.all(sites.map(async (site) => {
        const folder = vscode.workspace.getWorkspaceFolder(site.source)!
        const selected = await selectSite(
          folder,
          vscode.Uri.file(path.join(folder.uri.fsPath, 'selected config.toml')),
        )
        assert.ok(selected)
        return { ...selected, folder: { ...folder, name: 'Same site name' } }
      }))
      const provider = new SiteTasks(() => selections)
      const tasks = provider.provideTasks()
      assert.equal(tasks.length, 2)
      for (const task of tasks) {
        assert.equal(typeof task.scope, 'object')
        const folder = task.scope as vscode.WorkspaceFolder
        const resolved = provider.resolveTask(task)
        assert.ok(resolved?.execution instanceof vscode.ProcessExecution)
        assert.equal(resolved.execution.options?.cwd, folder.uri.fsPath)
        assert.equal(
          resolved.execution.args[2],
          selections.find((selection) => selection.folder.uri.toString() === folder.uri.toString())!
            .config.fsPath,
        )
      }
    }))

  await check('Source status excludes foreign diagnostics', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      await workbenchText({ status: '(?<!not )checked' })
      const foreign = vscode.languages.createDiagnosticCollection('unrelated-provider')
      try {
        foreign.set(site.source, [
          new vscode.Diagnostic(
            new vscode.Range(0, 0, 0, 1),
            'Another provider reports this error',
            vscode.DiagnosticSeverity.Error,
          ),
        ])
        const status = await workbenchText({ status: '(?<!not )checked', capture: true })
        assert.doesNotMatch(status, /diagnostic|failed/)
      } finally {
        foreign.dispose()
      }
    }))

  await check('Source warnings retain checked status', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      await waitForDiagnostics(site.source, (diagnostics) =>
        diagnostics.some((diagnostic) =>
          diagnostic.source === 'tola' && diagnostic.severity === vscode.DiagnosticSeverity.Warning
        ))
      const warnings = vscode.languages.getDiagnostics(site.source).filter((diagnostic) =>
        diagnostic.source === 'tola' && diagnostic.severity === vscode.DiagnosticSeverity.Warning
      )
      // The compiler's own warning and Tola's advice about the same font are both the author's to
      // read: a hint that says more than the report it overlaps is not dropped.
      const messages = new Set(warnings.map((warning) =>
        warning.message
      ))
      assert.equal(messages.size, 2, JSON.stringify([...messages]))
      const status = await workbenchText({ status: 'checked \\(', capture: true })
      assert.doesNotMatch(status, /not checked|failed/)
      assert.equal(status.match(/\((\d+) diagnostic/)?.[1], '2')
    }, { content: '#set text(font: "Tola E2E absent font")\nWarning-bearing source.\n' }))

  await check('Failed sources retain check feedback', () =>
    withPreview(() =>
      withSite(async (site) => {
        await vscode.window.showTextDocument(site.source)
        await vscode.commands.executeCommand('tola.startPreview', site.source)
        const module = await vscode.workspace.openTextDocument(
          vscode.Uri.file(path.join(site.root, 'module.typ')),
        )
        await vscode.window.showTextDocument(module)
        const program = await vscode.workspace.openTextDocument(site.program)
        await replace(program, '#panic("Unpublished root failure")\n')
        await waitForDiagnostics(program.uri, (diagnostics) =>
          diagnostics.some((diagnostic) => diagnostic.severity === vscode.DiagnosticSeverity.Error))
        const status = await workbenchText({ status: 'failed.*No published page', capture: true })
        assert.match(status, /failed/)
      }, {
        prepare: (site) =>
          fs.writeFile(path.join(site.root, 'module.typ'), '#let original = 1\n'),
      })
    ))

  await check('Check mode changes without restarting services', () =>
    withSite(async (site) => {
      const service = await recordServiceStarts(site)
      const program = await vscode.workspace.openTextDocument(site.program)
      await vscode.window.showTextDocument(program)
      await symbols(program.uri)
      await workbenchText({ status: '(?<!not )checked' })
      await vscode.commands.executeCommand('tolaPages.focus')
      await workbenchText({ pages: '/preview/' })
      const settings = vscode.workspace.getConfiguration('tola', site.source)
      await settings.update('checkMode', 'onSave', vscode.ConfigurationTarget.WorkspaceFolder)
      await symbols(program.uri)
      await replace(
        program,
        '#document("updated/index.html", format: "html")[#include "content/page.typ"]\n',
      )
      await workbenchText({ status: 'not checked', capture: true })
      await workbenchText({ pagesMessage: 'Source changes not checked', capture: true })
      assert.equal(await program.save(), true)
      const saved = await workbenchText({ pages: '/updated/', capture: true })
      assert.doesNotMatch(saved, /\/preview\//)
      await settings.update('checkMode', 'onType', vscode.ConfigurationTarget.WorkspaceFolder)
      await symbols(program.uri)
      await replace(
        program,
        '#document("live/index.html", format: "html")[#include "content/page.typ"]\n',
      )
      const current = await workbenchText({ pages: '/live/', capture: true })
      assert.doesNotMatch(current, /\/updated\//)
      await workbenchText({ status: '(?<!not )checked' })
      assert.equal(await service.starts('lsp'), 1)
    }))

  await check('Selected config edits clear diagnostics', () =>
    withSite(async (site) => {
      const document = await vscode.workspace.openTextDocument(site.configuration)
      const saved = document.getText()
      await replace(document, '[site\n')
      await waitForDiagnostics(site.configuration, (diagnostics) =>
        diagnostics.some((diagnostic) =>
          diagnostic.severity === vscode.DiagnosticSeverity.Error
        ))
      await replace(document, saved)
      await waitForDiagnostics(site.configuration, (diagnostics) => diagnostics.length === 0)
      assert.equal(await fs.readFile(site.configuration.fsPath, 'utf8'), saved)
    }))

  await check('Configuration fixes apply in the editor', () =>
    withSite(async (site) => {
      const { document, position } = await openMarked(
        site.configuration,
        '[site]\ntitle = "Editor contract"\ndescript|io = "Typo"\n',
      )
      const actions = await vscode.commands.executeCommand<(vscode.CodeAction | vscode.Command)[]>(
        'vscode.executeCodeActionProvider',
        document.uri,
        new vscode.Range(position, position),
      )
      const fix = actions.find((action): action is vscode.CodeAction =>
        'kind' in action && action.command !== undefined &&
        action.kind?.value === vscode.CodeActionKind.QuickFix.value
      )
      assert.ok(fix?.command, 'The misspelled configuration key has no applicable fix')
      assert.equal(
        await vscode.commands.executeCommand(fix.command.command, ...(fix.command.arguments ?? [])),
        true,
      )
      assert.match(document.getText(), /\ndescription = "Typo"/)
      await waitForDiagnostics(document.uri, (diagnostics) => diagnostics.length === 0)
    }))
  await check('CLI operations show background progress', async () => {
    if (process.platform === 'win32') return
    for (
      const [command, label, confirmation] of [
        ['tola.build', 'Building', undefined],
        ['tola.editorSetup', 'Set Up Editor Integration', 'Set Up Integration'],
      ] as const
    ) {
      await withSite(async (site) => {
        await vscode.window.showTextDocument(site.source)
        const held = await holdServerStart(site)
        await held.reached
        const progresses: vscode.ProgressOptions[] = []
        const notices: string[] = []
        const progress = vscode.window.withProgress
        const information = vscode.window.showInformationMessage
        const error = vscode.window.showErrorMessage
        vscode.window.withProgress = ((options: vscode.ProgressOptions, ...args: unknown[]) => {
          progresses.push(options)
          return Reflect.apply(progress, vscode.window, [options, ...args])
        }) as typeof progress
        vscode.window.showInformationMessage = ((message: string, ...args: unknown[]) => {
          notices.push(message)
          return Reflect.apply(information, vscode.window, [message, ...args])
        }) as typeof information
        vscode.window.showErrorMessage = ((message: string, ...args: unknown[]) => {
          notices.push(message)
          return Reflect.apply(error, vscode.window, [message, ...args])
        }) as typeof error
        let completed = false
        const running = vscode.commands.executeCommand(command).then(() => completed = true)
        const started = waitForOutput(`Started Tola language service for ${site.name}.`)
        try {
          if (confirmation) await workbenchText({ button: confirmation })
          await workbenchText({ status: `Tola: ${label}` })
          assert.ok(
            progresses.some((options) =>
              options.location === vscode.ProgressLocation.Window &&
              options.title?.startsWith(`Tola: ${label}`)
            ),
          )
          assert.equal(completed, false)
          await held.release()
          await running
          assert.deepEqual(notices, [])
        } finally {
          await held.release()
          await running
          await started
          await held.close()
          vscode.window.withProgress = progress
          vscode.window.showInformationMessage = information
          vscode.window.showErrorMessage = error
        }
      })
    }
  })

  await check('Build publishes the selected site', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      await vscode.commands.executeCommand('tola.build')
      assert.match(
        await fs.readFile(path.join(site.root, 'public/preview/index.html'), 'utf8'),
        /Published editor page/,
      )
    }))

  await check('Build survives stopped language services', () =>
    withSite(async (site) => {
      const service = await recordServiceStarts(site)
      await symbols(site.source)
      const pid = await service.servicePid('lsp')
      assert.ok(pid)
      process.kill(pid, 'SIGKILL')
      await workbenchText({ button: 'Show Output' })
      await vscode.commands.executeCommand('workbench.action.closeAllEditors')
      await vscode.commands.executeCommand('tola.build')
      assert.ok((await fs.stat(path.join(site.root, 'public/preview/index.html'))).isFile())
    }))

  await check('Custom missing configuration stays selected', () =>
    withSite(async (site) => {
      const missing = vscode.Uri.file(path.join(site.root, 'missing.toml'))
      await vscode.workspace.getConfiguration('tola', site.source)
        .update('configPath', 'missing.toml', vscode.ConfigurationTarget.WorkspaceFolder)
      await vscode.commands.executeCommand('tola.restart')
      assert.equal(surfaces.siteSelected(), true)
      const [task] = await vscode.tasks.fetchTasks({ type: 'tola' })
      assert.ok(task?.execution instanceof vscode.ProcessExecution)
      assert.equal(task.execution.args[2], missing.fsPath)
      await vscode.workspace.openTextDocument(site.source)
      await waitForDiagnostics(missing, (messages) =>
        messages.some((message) => message.severity === vscode.DiagnosticSeverity.Error))
      await fs.writeFile(missing.fsPath, '[site]\ntitle = "Restored site"\n')
      await waitForDiagnostics(missing, (messages) =>
        messages.length === 0)
    }))

  await check('Create Site writes current CLI scaffold', () =>
    withSite(async (workspace) => {
      const directory = vscode.Uri.file(path.join(workspace.root, 'created'))
      await fs.mkdir(directory.fsPath)
      const open = vscode.window.showOpenDialog
      vscode.window.showOpenDialog = (() => Promise.resolve([directory])) as typeof open
      try {
        const creating = vscode.commands.executeCommand('tola.initSite')
        await workbenchText({ button: 'Create Site' })
        await creating
        const configuration = await fs.readFile(path.join(directory.fsPath, 'tola.toml'), 'utf8')
        assert.match(configuration, /\[site\]/)
        assert.ok((await fs.stat(path.join(directory.fsPath, 'site.typ'))).isFile())
        const settings = JSON.parse(
          await fs.readFile(path.join(directory.fsPath, '.vscode/settings.json'), 'utf8'),
        )
        assert.ok(settings['tola.configPath'])
      } finally {
        vscode.window.showOpenDialog = open
      }
    }, { documents: true }))

  await check('Failed build names the site', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      await vscode.commands.executeCommand('tola.build')
      const message = await workbenchText({ error: true, dismiss: true })
      assert.ok(message.includes(`could not build ${site.name}`), message)
    }, {
      program: '#document("preview/index.html", format: "html")[#panic("The build refuses this page")]\n',
    }))

  await check('Reveal Output without a build warns', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      await vscode.commands.executeCommand('tola.revealOutput')
      const message = await workbenchText({ contains: 'has not built this site yet', dismiss: true })
      assert.match(message, /run Tola: Build Site/)
    }))

  await check("Build task runs the site's own CLI", () =>
    withSite(async (site) => {
      const [task, ...rest] = await vscode.tasks.fetchTasks({ type: 'tola' })
      assert.ok(task && !rest.length, 'An enabled site contributes exactly one Tola task')
      assert.equal(task.definition.verb, 'build')
      assert.equal((task.scope as vscode.WorkspaceFolder).uri.fsPath, site.root)
      assert.ok(task.execution instanceof vscode.ProcessExecution, 'The task runs the site CLI')
      const execution = task.execution
      assert.equal(execution.process, process.env.TOLA_TEST_BINARY)
      assert.deepEqual(execution.args.slice(0, 2), ['build', '--config'])
      assert.equal(
        await fs.realpath(execution.args[2]!),
        site.configuration.fsPath,
        'The task builds the site the commands build',
      )
    }))

  await check('Declined vendor writes nothing', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      const running = vscode.commands.executeCommand('tola.vendorPackages')
      await workbenchText({ contains: 'Vendor the packages', dismiss: true })
      await running
      const written = (await fs.readdir(site.root)).filter((entry) => entry.startsWith('vendor'))
      assert.deepEqual(written, [], 'A dismissed confirmation must leave the site untouched')
    }))

  await check('Discovered package roots keep client ownership', async () => {
    for (const count of [1, 2]) {
      let runner: string
      let source: vscode.Uri
      await withSites(count, async (sites) => {
        await Promise.all(sites.map((site) =>
          vscode.workspace.getConfiguration('tola', site.source)
            .update('serverPath', runner, vscode.ConfigurationTarget.WorkspaceFolder)
        ))
        await vscode.commands.executeCommand('tola.restart')
        for (const site of sites) {
          const { document, position } = await openMarked(
            site.source,
            '#import "@preview/tola-editor-owned:0.1.0": exported\n#export|ed\n',
          )
          const targets = await within(
            definitionTargets(document, position),
            10_000,
            'Discovered package navigation did not settle',
          )
          assert.equal(targets[0]?.toString(), source.toString(), `import target for ${site.name}`)
          assert.ok(assetDrops)
          const token = new vscode.CancellationTokenSource()
          try {
            const offered = await assetDrops.provideDocumentDropEdits(
              document,
              position,
              fileTransfer('scope.bin', () => Promise.resolve(new Uint8Array([255]))),
              token.token,
            )
            assert.ok(offered?.length, 'Author sources retain their site publishing action')
          } finally {
            token.dispose()
          }
        }
        const { document, position } = await openMarked(source!, '#let exported = 1\n#export|ed\n')
        const targets = await within(
          definitionTargets(document, position),
          10_000,
          'Discovered package navigation did not settle',
        )
        if (count === 2) {
          assert.deepEqual(targets, [], 'Shared external sources have no unique site')
          const stopped = waitForOutput(`Language service for ${sites[1]!.name} exited`)
          await vscode.workspace.getConfiguration('tola', sites[1]!.source)
            .update('enabled', false, vscode.ConfigurationTarget.WorkspaceFolder)
          await stopped
        } else {
          assert.equal(
            targets[0]?.toString(),
            source.toString(),
            'first definition in the opened cache source',
          )
        }
        const owned = await within(
          definitionTargets(document, position),
          10_000,
          'Discovered package navigation did not settle',
        )
        assert.equal(owned[0]?.toString(), source.toString(), 'definition from the unique remaining site')
        assert.ok(assetDrops)
        const cancellation = new vscode.CancellationTokenSource()
        try {
          assert.equal(
            await assetDrops.provideDocumentDropEdits(
              document,
              position,
              fileTransfer('dependency.bin', () => Promise.resolve(new Uint8Array([255]))),
              cancellation.token,
            ),
            undefined,
          )
        } finally {
          cancellation.dispose()
        }
        const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
          'vscode.executeCompletionItemProvider',
          document.uri,
          position,
        )
        assert.ok(
          completions.items.some((item) =>
            (typeof item.label === 'string' ? item.label : item.label.label) === 'exported'
          ),
        )
      }, {
        prepare: async (site) => {
          const root = path.dirname(site.root)
          const data = path.join(root, 'discovered-package-data')
          const cache = path.join(root, 'discovered-package-cache')
          const library = path.join(cache, 'preview/tola-editor-owned/0.1.0')
          await fs.mkdir(data)
          await fs.mkdir(library, { recursive: true })
          await fs.writeFile(
            path.join(library, 'typst.toml'),
            '[package]\nname="tola-editor-owned"\nversion="0.1.0"\nentrypoint="lib.typ"\n',
          )
          source = vscode.Uri.file(path.join(library, 'lib.typ'))
          await fs.writeFile(source.fsPath, '#let exported = 1\n')
          runner = path.join(root, 'discovered-packages.sh')
          await fs.writeFile(
            runner,
            `#!/bin/sh\nexport TYPST_PACKAGE_PATH=${quoted(data)}\nexport TYPST_PACKAGE_CACHE_PATH=${
              quoted(cache)
            }\nexec ${quoted(process.env.TOLA_TEST_BINARY!)} "$@"\n`,
            { mode: 0o755 },
          )
        },
      })
    }
  })

  await check('Configured external roots answer language features', async () => {
    for (const kind of ['site', 'package']) {
      let source: vscode.Uri
      await withSite(async () => {
        const { document, position } = await openMarked(
          source!,
          '#let external-value = 1\n#external-|value\n',
        )
        const targets = await definitionTargets(document, position)
        assert.ok(targets.some((target) => target.toString() === source.toString()), `${kind}: no definition`)
        const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
          'vscode.executeCompletionItemProvider',
          document.uri,
          position,
        )
        assert.ok(
          completions.items.some((item) =>
            (typeof item.label === 'string' ? item.label : item.label.label) === 'external-value'
          ),
          `${kind}: no completion`,
        )
      }, {
        prepare: async (site) => {
          const root = path.join(path.dirname(site.root), 'external roots')
          const selected = path.join(site.root, '.vscode/settings.json')
          const settings = JSON.parse(await fs.readFile(selected, 'utf8'))
          if (kind === 'site') {
            await fs.mkdir(root, { recursive: true })
            const config = path.join(root, 'tola.toml')
            await fs.writeFile(config, '')
            await fs.writeFile(path.join(root, 'site.typ'), '#document("index.html")[External site]')
            settings['tola.configPath'] = config
            source = vscode.Uri.file(path.join(root, 'source.typ'))
          } else {
            const library = path.join(root, 'local/library/0.1.0')
            await fs.mkdir(library, { recursive: true })
            await fs.writeFile(
              path.join(library, 'typst.toml'),
              '[package]\nname="library"\nversion="0.1.0"\nentrypoint="lib.typ"\n',
            )
            settings['tola.packagePath'] = root
            source = vscode.Uri.file(path.join(library, 'lib.typ'))
          }
          await fs.writeFile(source.fsPath, '#let external-value = 1\n')
          await fs.writeFile(selected, JSON.stringify(settings))
        },
      })
    }
  })

  await check('Empty definitions stay in the editor', () =>
    withSite(async (site) => {
      const { document, position } = await openMarked(site.source, '#let count = 1\n | \n')
      const editor = await vscode.window.showTextDocument(document)
      editor.selection = new vscode.Selection(position, position)
      const messages: string[] = []
      const show = vscode.window.showInformationMessage
      vscode.window.showInformationMessage = ((message: string, ...args: unknown[]) => {
        messages.push(message)
        return Reflect.apply(show, vscode.window, [message, ...args])
      }) as typeof show
      try {
        await vscode.commands.executeCommand('tola.goToDefinition')
        assert.equal(vscode.window.activeTextEditor?.document, document)
        assert.deepEqual(messages, [])
      } finally {
        vscode.window.showInformationMessage = show
      }
    }))

  await check('Definition commands preserve package ownership', () =>
    withSites(2, async (sites) => {
      for (const site of sites) {
        const { document, position, uri } = await packageDefinition(site)
        const editor = await vscode.window.showTextDocument(document)
        editor.selection = new vscode.Selection(position, position)
        let changed: vscode.Disposable | undefined
        const navigated = new Promise<void>((resolve) => {
          changed = vscode.window.onDidChangeActiveTextEditor((active) => {
            if (active?.document.uri.toString() === uri.toString()) resolve()
          })
        })
        try {
          await vscode.commands.executeCommand('tola.goToDefinition')
          await within(navigated, 10_000, 'Definition command did not open its package target')
          assert.equal(vscode.window.activeTextEditor?.document.uri.toString(), uri.toString())
          assert.equal((await typstDocument(uri)).languageId, 'typst')
        } finally {
          changed?.dispose()
        }
      }
    }))

  await check('Edited definitions leave navigation unchanged', () =>
    withSite(async (site) => {
      const held = await blockRequest(site, 'textDocument/definition')
      try {
        const { document, position } = await openMarked(site.source, packageImport)
        const editor = await vscode.window.showTextDocument(document)
        editor.selection = new vscode.Selection(position, position)
        const navigating = vscode.commands.executeCommand('tola.goToDefinition')
        await within(held.reached, 10_000, 'Definition did not reach the service gate')
        await replace(document, '#let replacement = 2\n#replacement\n')
        held.release()
        await navigating
        assert.equal(vscode.window.activeTextEditor?.document, document)
        assert.equal(document.getText(), '#let replacement = 2\n#replacement\n')
      } finally {
        await held.close()
      }
    }))

  await check('Path completions honor insert and replace modes', () =>
    withSite(async (site) => {
      const settings = vscode.workspace.getConfiguration('editor', site.source)
      for (const [mode, expected] of [['insert', 'parts/tail.typ'], ['replace', 'parts/']]) {
        await settings.update(
          'suggest.insertMode',
          mode === 'insert' ? undefined : mode,
          vscode.ConfigurationTarget.WorkspaceFolder,
        )
        assert.equal(vscode.workspace.getConfiguration('editor', site.source).get('suggest.insertMode'), mode)
        const { document, position } = await openMarked(site.source, '#include "pa|tail.typ"\n')
        const editor = await vscode.window.showTextDocument(document)
        editor.selection = new vscode.Selection(position, position)
        await vscode.commands.executeCommand('editor.action.triggerSuggest')
        await workbenchText({ completion: 'parts/' })
        await vscode.commands.executeCommand('acceptSelectedSuggestion')
        assert.equal(document.lineAt(0).text, `#include "${expected}"`)
        assert.equal(await document.save(), true)
        assert.equal(await fs.readFile(document.uri.fsPath, 'utf8'), document.getText())
      }
    }, {
      prepare: async (site) => {
        const directory = path.join(site.root, 'content/parts')
        await fs.mkdir(directory)
        await fs.writeFile(path.join(directory, 'tail.typ'), 'Tail\n')
      },
    }))

  await check('Call completion edits reach saved sources', () =>
    withSite(async (site) => {
      const settings = vscode.workspace.getConfiguration('editor', site.source)
      await settings.update('suggest.insertMode', 'replace', vscode.ConfigurationTarget.WorkspaceFolder)
      const { document, position } = await openMarked(
        site.source,
        '#let completed-call(value) = [#value]\n#completed-ca|zz\n',
      )
      const editor = await vscode.window.showTextDocument(document)
      editor.selection = new vscode.Selection(position, position)
      const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
        'vscode.executeCompletionItemProvider',
        document.uri,
        position,
      )
      const call = completions.items.find((item) =>
        (typeof item.label === 'string' ? item.label : item.label.label) === 'completed-call'
      )
      assert.ok(call?.insertText instanceof vscode.SnippetString)
      assert.ok(call.range && 'inserting' in call.range && 'replacing' in call.range)
      assert.ok(call.range.replacing.end.isAfter(call.range.inserting.end))
      await vscode.commands.executeCommand('editor.action.triggerSuggest')
      await workbenchText({ completion: 'completed-call' })
      await vscode.commands.executeCommand('acceptSelectedSuggestion')
      await vscode.commands.executeCommand('type', { text: '42' })
      await vscode.commands.executeCommand('leaveSnippet')
      assert.equal(document.lineAt(1).text, '#completed-call(42)')
      assert.equal(await document.save(), true)
      assert.equal(await fs.readFile(document.uri.fsPath, 'utf8'), document.getText())
    }))

  await check('Definitions open read-only Typst', () =>
    withSite(async (site) => {
      const { uri } = await packageDefinition(site)
      assert.equal(uri.scheme, 'tola-package')
      const source = await typstDocument(uri)
      await vscode.window.showTextDocument(source)
      const before = source.getText()
      await vscode.commands.executeCommand('type', { text: 'changed' })
      assert.equal(source.getText(), before)
      assert.equal(source.isDirty, false)
    }))

  await check('Renames edit unsaved UTF-16 sources', () =>
    withSite(async (site) => {
      const { document, position } = await openMarked(
        site.source,
        '#let greeting(name: "world") = [Hello, #name.]\n𝄞 #greet|ing()\n',
      )
      const rename = await vscode.commands.executeCommand<vscode.WorkspaceEdit>(
        'vscode.executeDocumentRenameProvider',
        document.uri,
        position,
        'welcome',
      )
      assert.ok(rename, 'The named function has no rename edit')
      assert.equal(await vscode.workspace.applyEdit(rename), true)
      assert.equal(document.getText(), '#let welcome(name: "world") = [Hello, #name.]\n𝄞 #welcome()\n')
      assert.equal(await fs.readFile(site.source.fsPath, 'utf8'), 'Published editor page.\n')
    }))

  await check('Language feedback leaves disk untouched', async () => {
    let settingsBefore = ''
    await withSite(async (site) => {
      const { document, position } = await openMarked(site.source, '𝄞 #lo|rem(1)\n')
      const hover = await vscode.commands.executeCommand<vscode.Hover[]>(
        'vscode.executeHoverProvider',
        document.uri,
        position,
      )
      assert.ok(hover[0], 'The selected site has no language service')
      assert.equal(await fs.readFile(path.join(site.root, '.vscode/settings.json'), 'utf8'), settingsBefore)
      await assert.rejects(fs.stat(path.join(site.root, '.tola')), { code: 'ENOENT' })
    }, {
      prepare: async (site) => {
        settingsBefore = await fs.readFile(path.join(site.root, '.vscode/settings.json'), 'utf8')
      },
    })
  })

  await check(
    'Disabled site refuses its own package documents',
    () =>
      withSites(2, async ([disabled, running]) => {
        assert.ok(disabled && running, 'The case runs against two sites')
        const { uri: refused } = await packageDefinition(disabled)
        const { uri: kept } = await packageDefinition(running)
        assert.notEqual(
          refused.toString(),
          kept.toString(),
          'Both sites answered with the same package document',
        )
        const keptSource = (await typstDocument(kept)).getText()

        await disable(disabled)
        await assert.rejects(async () => {
          await vscode.workspace.openTextDocument(refused)
        }, (error: unknown) => {
          const message = error instanceof Error ? error.message : String(error)
          assert.match(message, /not running/)
          assert.ok(message.includes(disabled.name), `The refusal does not name ${disabled.name}: ${message}`)
          return true
        })
        // The running site's own document keeps answering the source it answered before.
        assert.equal((await typstDocument(kept)).getText(), keptSource)
      }),
  )

  await check('A package document without a site is refused', () =>
    withSite(async () => {
      const plain = vscode.Uri.parse('tola-package:/tola/document/0.0.0/lib.typ')
      await assert.rejects(async () => {
        await vscode.workspace.openTextDocument(plain)
      }, { message: /Invalid Tola package URI/ })
    }))

  await check('Restart keeps a package document with its site', () =>
    withSites(2, async ([site, other]) => {
      assert.ok(site && other, 'The case runs against two sites')
      const { document, position, uri } = await packageDefinition(site)
      const definition = await typstDocument(uri)
      const source = definition.getText()

      await vscode.commands.executeCommand('tola.restart')
      const [restarted] = await definitionTargets(document, position)
      assert.ok(restarted, 'The restarted client answers no package document')
      assert.equal(
        restarted.toString(),
        uri.toString(),
        "A restarted client answered with another site's package document",
      )

      // Closing the document drops the copy the site rendered, so reopening it reads that client again.
      await vscode.window.showTextDocument(definition)
      await vscode.commands.executeCommand('workbench.action.closeActiveEditor')
      assert.equal(
        (await typstDocument(uri)).getText(),
        source,
        "The reopened document no longer reads its site's source",
      )

      // With the other site gone, only the site the URI names can answer it.
      await disable(other)
      assert.equal(
        (await typstDocument(uri)).getText(),
        source,
        'The document changed when another site stopped',
      )
    }))

  await check(
    'Untitled buffers answer from the site they choose',
    () =>
      withSites(2, async ([first, second]) => {
        assert.ok(first && second, 'The case runs against two sites')
        await withSiteChoice((labels) => labels.find((label) => label === second.name), async (offered) => {
          const buffer = await vscode.workspace.openTextDocument({ language: 'typst', content: '= Draft\n' })
          await vscode.window.showTextDocument(buffer)
          assert.equal(
            (await symbols(buffer.uri)).length,
            1,
            'The chosen site answered no outline for the untitled buffer',
          )
          assert.deepEqual(offered, [[first.name, second.name]])

          // Working in another site's source must not move the buffer's site.
          await vscode.window.showTextDocument(first.source)
          await disable(first)
          assert.equal((await symbols(buffer.uri)).length, 1, 'The untitled buffer followed another site')

          // Only the site the buffer belongs to answers it: no other client takes over when it stops.
          await disable(second)
          assert.equal(
            (await symbols(buffer.uri)).length,
            0,
            'Another site answered the buffer of a stopped site',
          )
        })
      }),
  )

  await check('Declined site choice leaves untitled buffers unattached', () =>
    withSites(2, async () => {
      await withSiteChoice(() => undefined, async (offered) => {
        const buffer = await vscode.workspace.openTextDocument({ language: 'typst', content: '= Draft\n' })
        await vscode.window.showTextDocument(buffer)
        assert.equal((await symbols(buffer.uri)).length, 0, 'An unattached buffer answered language features')

        const other = await vscode.workspace.openTextDocument({ language: 'typst', content: '= Other\n' })
        await vscode.window.showTextDocument(other)
        assert.equal((await symbols(other.uri)).length, 0, 'An unattached buffer answered language features')
        assert.equal(offered.length, 2, 'Each untitled buffer asks for its own site once')
      })
    }))

  await check(
    'Save As moves an untitled buffer into its site',
    () =>
      withSites(2, async ([first, second]) => {
        assert.ok(first && second, 'The case runs against two sites')
        await withSiteChoice((labels) => labels.find((label) => label === first.name), async (offered) => {
          // An untitled document with an associated path is what Save As saves: the editor writes that
          // path when the document is saved, without asking for a location.
          const destination = vscode.Uri.file(path.join(second.root, 'content/scratch.typ'))
          const buffer = await vscode.workspace.openTextDocument(destination.with({ scheme: 'untitled' }))
          await vscode.window.showTextDocument(buffer)
          assert.equal(
            buffer.languageId,
            'typst',
            'The associated path did not select Typst for the untitled buffer',
          )
          await replace(buffer, '= Draft\n')
          assert.equal(
            (await symbols(buffer.uri)).length,
            1,
            'The chosen site answered no outline for the untitled buffer',
          )
          assert.deepEqual(
            offered,
            [[first.name, second.name]],
            'One untitled buffer asked for its site once',
          )

          const closed = waitForClose(buffer.uri)
          assert.equal(await buffer.save(), true, 'Saving the untitled buffer failed')
          await closed
          assert.equal(await fs.readFile(destination.fsPath, 'utf8'), '= Draft\n')
          // The file the buffer became answers from the site whose folder holds it, not from the
          // buffer's site.
          const document = await typstDocument(destination)
          assert.equal(
            (await symbols(document.uri)).length,
            1,
            'The site whose folder holds the saved file answered no outline',
          )
        })
      }),
  )

  await check('A package document opens while its service starts', async () => {
    if (process.platform === 'win32') return
    await withSite(async (site) => {
      const { uri } = await packageDefinition(site)
      // Read and close the document first, so the read below asks this site again. The editor opens
      // it, because a document read through `workspace.openTextDocument` stays open past its editor,
      // and the close is awaited from before the editor closes, the way the save-as case does it.
      await vscode.commands.executeCommand('vscode.open', uri)
      const read = vscode.workspace.textDocuments.find((candidate) =>
        candidate.uri.toString() === uri.toString()
      )
      assert.ok(read, 'The package document did not open in the editor')
      const source = read.getText()
      const closed = waitForClose(uri)
      await vscode.commands.executeCommand('workbench.action.closeActiveEditor')
      await closed

      // The site's next service is held before it runs, so the request below meets a service that is
      // starting and only the client that comes up can answer it.
      const blocked = await blockServerStart(site)
      try {
        const opening = vscode.workspace.openTextDocument(uri)
        await blocked.release()
        await opening
        assert.equal(
          (await typstDocument(uri)).getText(),
          source,
          'The document opened while its service started read another source',
        )
      } finally {
        await blocked.close()
      }
    })
  })

  await check('A package request waits for its own site alone', async () => {
    if (process.platform === 'win32') return
    await withSites(2, async ([site, other]) => {
      assert.ok(site && other, 'The case runs against two sites')
      const { uri } = await packageDefinition(site)
      // Read and close the document first, so the request below asks the site's provider again.
      await vscode.commands.executeCommand('vscode.open', uri)
      const read = vscode.workspace.textDocuments.find((candidate) =>
        candidate.uri.toString() === uri.toString()
      )
      assert.ok(read, 'The package document did not open in the editor')
      const source = read.getText()
      const closed = waitForClose(uri)
      await vscode.commands.executeCommand('workbench.action.closeActiveEditor')
      await closed

      // The other site's handshake is held, so the transition that restarted it cannot settle. Its
      // gate is never opened during the case: only a transition that restarts this site's service
      // too, while the other site is held, can reach the wait under test.
      const heldOther = await holdServerStart(other)
      let heldSite: HeldStart | undefined
      let opening: Promise<vscode.TextDocument> | undefined
      try {
        await heldOther.reached
        // Gating this site's service cancels the held handshake of the other site, the way any
        // restart does, so the next transition starts both sites: this one held too, and the
        // other still held from the previous one.
        heldSite = await holdServerStart(site)
        await heldSite.reached
        opening = Promise.resolve(vscode.workspace.openTextDocument(uri))
        // The site has no running client while its gate is held, so only the client that comes up
        // can answer this request. The open crosses the workbench first, which gives the request
        // the moment to reach the extension before the gate opens.
        assert.equal(
          await Promise.race([
            opening.then(() => 'opened' as const, () => 'failed' as const),
            timedOut(2_000),
          ]),
          'timeout',
          'A package request answered while its own site was held',
        )
        const started = waitForOutput(`Started Tola language service for ${site.name}.`)
        await heldSite.release()
        await started
        // This site is now ready while the other site is still in its handshake. Its package
        // request must answer from the site's client, not wait for the other handshake to finish.
        const document = await within(
          opening,
          5_000,
          "A package request for a ready site waited for another site's handshake",
        )
        assert.equal(document.getText(), source, "The package request read another site's source")
      } finally {
        // An open gate lets every later start run, so the transitions this case leaves queued
        // finish even though the executable below is the real one again.
        await heldSite?.release()
        await heldOther.release()
        await vscode.workspace.getConfiguration('tola', site.source)
          .update('serverPath', process.env.TOLA_TEST_BINARY, vscode.ConfigurationTarget.WorkspaceFolder)
        await vscode.workspace.getConfiguration('tola', other.source)
          .update('serverPath', process.env.TOLA_TEST_BINARY, vscode.ConfigurationTarget.WorkspaceFolder)
        await vscode.commands.executeCommand('tola.restart')
        await heldSite?.dispose()
        await heldOther.dispose()
        await opening?.catch(() => {})
      }
    })
  })

  await runEnterChecks()
  await runPreviewChecks()
  console.log('Tola VS Code extension checks passed.')
}
