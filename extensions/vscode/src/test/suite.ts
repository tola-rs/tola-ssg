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
import {
  blockRequest,
  blockServerStart,
  check,
  definitionTargets,
  disable,
  type EditorSite,
  openMarked,
  openPublishedPage,
  recordOutputChannels,
  recordServiceMessages,
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

export async function run(): Promise<void> {
  const extension = vscode.extensions.getExtension('tola.tola')
  recordOutputChannels()
  const surfaces = recordSiteSurfaces()
  assert.ok(extension, 'Tola extension was not loaded')
  await extension.activate()

  await runGrammarChecks(extension.extensionPath)

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
      assert.match(await workbenchText({ error: true, dismiss: true }), /no longer current/)
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

  await check('Stopped services offer restart', () =>
    withSite(async (site) => {
      const recorded = waitForOutput('Started Tola language service for')
      const service = await recordServiceMessages(site)
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
    }, { content: '= Restored heading\n' }))

  await check('Restart cancels pending initialization', async () => {
    if (process.platform === 'win32') return
    await withSite(async (site) => {
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
  })

  await check('Closing cancels pending initialization', async () => {
    if (process.platform === 'win32') return
    await withSite(async (site) => {
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
  })

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
      assert.equal(warnings.length, 1)
      const status = await workbenchText({ status: 'checked \\(', capture: true })
      assert.doesNotMatch(status, /not checked|failed/)
      assert.equal(status.match(/\((\d+) diagnostic/)?.[1], '1')
    }, { content: '#set text(font: "Tola E2E absent font")\nWarning-bearing source.\n' }))

  await check('Failed sources retain check feedback', () =>
    withPreview(() =>
      withSite(async (site) => {
        await vscode.window.showTextDocument(site.source)
        await vscode.commands.executeCommand('tola.startPreview', site.source)
        await workbenchText({ contains: `Tola published ${site.name}`, dismiss: true })
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

  await check('On-save Pages mark unchecked routes', () =>
    withSite(async (site) => {
      const service = await blockRequest(site, 'tola/onEnter', 'onSave')
      try {
        const program = await vscode.workspace.openTextDocument(site.program)
        await vscode.window.showTextDocument(program)
        await vscode.commands.executeCommand('tolaPages.focus')
        await workbenchText({ pages: '/preview/' })
        await replace(
          program,
          '#document("updated/index.html", format: "html")[#include "content/page.typ"]\n',
        )
        await workbenchText({ status: 'not checked', capture: true })
        const stale = await workbenchText({ pagesMessage: 'Source changes not checked', capture: true })
        assert.match(stale, /\/preview\//)
        assert.doesNotMatch(stale, /\/updated\//)
        assert.equal(await program.save(), true)
        const current = await workbenchText({ pages: '/updated/', capture: true })
        assert.doesNotMatch(current, /\/preview\//)
        await workbenchText({ status: '(?<!not )checked' })
      } finally {
        await service.close()
      }
    }))

  await check(
    'Formatter width change keeps the open preview',
    () =>
      withPreview((opened) =>
        withSite(async (site) => {
          const service = await recordServiceMessages(site)
          try {
            await symbols(site.source)
            const started = await service.starts('lsp')
            await vscode.window.showTextDocument(site.source)
            await vscode.commands.executeCommand('tola.openPreview', site.source)
            assert.equal(opened[0]?.status, 200)
            await vscode.workspace.getConfiguration('tola', site.source)
              .update('formatterPrintWidth', 40, vscode.ConfigurationTarget.WorkspaceFolder)
            await service.waitForMessage('workspace/didChangeConfiguration')
            const sent = await service.messages()
            assert.match(sent.slice(sent.indexOf('workspace/didChangeConfiguration')), /"printWidth":40/)
            assert.equal(await service.starts('lsp'), started, 'The settings change restarted the service')
            const response = await fetch(opened[0].url, { signal: AbortSignal.timeout(15_000) })
            assert.equal(response.status, 200, 'The settings change stopped the preview server')
          } finally {
            await vscode.workspace.getConfiguration('tola', site.source)
              .update('serverPath', process.env.TOLA_TEST_BINARY, vscode.ConfigurationTarget.WorkspaceFolder)
          }
        })
      ),
  )

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
  await check('Build reports the site it published', () =>
    withSite(async (site) => {
      await vscode.window.showTextDocument(site.source)
      await vscode.commands.executeCommand('tola.build')
      const message = await workbenchText({ contains: `Tola built ${site.name}.`, dismiss: true })
      assert.ok(message.includes(site.name), message)
    }))

  await check('Build survives stopped language services', () =>
    withSite(async (site) => {
      const service = await recordServiceMessages(site)
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

  await runEnterChecks()
  await runPreviewChecks()
  console.log('Tola VS Code extension checks passed.')
}
