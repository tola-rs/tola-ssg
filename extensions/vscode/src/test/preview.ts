import * as assert from 'node:assert/strict'
import * as fs from 'node:fs/promises'
import type { ServerResponse } from 'node:http'
import * as path from 'node:path'
import * as vscode from 'vscode'
import { startGate } from '../../../../e2e/support/gate.ts'
import { SiteCli } from '../site-cli.ts'
import { selectSite } from '../selection.ts'
import {
  check,
  type EditorSite,
  openPublishedPage,
  replace,
  withPreview,
  withSite,
  withSites,
  workbenchText,
} from './site.ts'

async function withNotices(assertion: (messages: string[]) => Promise<void>): Promise<void> {
  const show = vscode.window.showInformationMessage
  const messages: string[] = []
  vscode.window.showInformationMessage = ((message: string, ...args: unknown[]) => {
    messages.push(message)
    return Reflect.apply(show, vscode.window, [message, ...args])
  }) as typeof show
  try {
    await assertion(messages)
  } finally {
    vscode.window.showInformationMessage = show
  }
}

type HeldPublication = {
  html: string
  devPid: number
  hookPid: number
  closed: Promise<void>
}

async function within<T>(promise: PromiseLike<T>, label: string): Promise<T> {
  let deadline: NodeJS.Timeout | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        deadline = setTimeout(() => reject(new Error(`${label} timed out`)), 20_000)
      }),
    ])
  } finally {
    clearTimeout(deadline)
  }
}

async function publicationGate(hold: (html: string) => boolean) {
  const responses: ServerResponse[] = []
  let released = false
  let reached!: (publication: HeldPublication) => void
  const observed = new Promise<HeldPublication>((resolve) => {
    reached = resolve
  })
  const gate = await startGate((request, response) => {
    let html = ''
    request.setEncoding('utf8')
    request.on('data', (chunk) => {
      html += chunk
    })
    request.on('end', () => {
      if (released || !hold(html)) {
        response.end('continue')
        return
      }
      responses.push(response)
      reached({
        html,
        devPid: Number(request.headers['x-dev-pid']),
        hookPid: Number(request.headers['x-hook-pid']),
        closed: new Promise((resolve) => response.once('close', resolve)),
      })
    })
  })
  return {
    next: () => within(observed, 'Candidate publication'),
    release: () => {
      released = true
      for (const response of responses) response.end('continue')
    },
    close: gate.close,
    prepare: async (site: EditorSite) => {
      const script = path.join(site.root, 'observe-publication.mjs')
      await fs.writeFile(
        script,
        `
        import { readFile, readdir, writeFile } from 'node:fs/promises';
        import { join } from 'node:path';
        const root = process.env.TOLA_HOOK_INPUT_DIR;
        const files = await readdir(root, { recursive: true });
        const pages = await Promise.all(files.filter(file => file.endsWith('.html'))
          .map(async file => file + '\\n' + await readFile(join(root, file), 'utf8')));
        const response = await fetch(${JSON.stringify(gate.url)}, {
          method: 'POST', body: pages.join('\\n'),
          headers: { 'x-dev-pid': String(process.ppid), 'x-hook-pid': String(process.pid) },
        });
        if (!response.ok) throw new Error('Publication gate rejected the candidate');
        await response.text();
        await writeFile(join(process.env.TOLA_HOOK_OUTPUT_DIR, 'observed.txt'), 'observed');
      `,
      )
      await fs.appendFile(
        site.configuration.fsPath,
        '\n[[build.hooks.generate-outputs]]\nname = "publication-gate"\n' +
          `command = ${JSON.stringify([process.env.TOLA_TEST_NODE, script])}\n` +
          'outputs = [{ file = "observed.txt" }]\n',
      )
    },
  }
}

function assertStopped(pid: number): void {
  assert.ok(Number.isInteger(pid) && pid > 0)
  assert.throws(
    () => process.kill(pid, 0),
    (error: unknown) => error instanceof Error && 'code' in error && error.code === 'ESRCH',
  )
}

export async function runPreviewChecks(): Promise<void> {
  await check('Preview starts from its own published address', async () => {
    let foreignRequests = 0
    const foreign = await startGate((_request, response) => {
      foreignRequests += 1
      response.setHeader('content-type', 'application/json')
      response.end(JSON.stringify({ revision: 'foreign', routes: [] }))
    })
    const gate = await publicationGate(() => true)
    try {
      await withSite(async (site) => {
        const recorded = path.join(site.root, '.tola/state/dev-server.json')
        await fs.mkdir(path.dirname(recorded), { recursive: true })
        await fs.writeFile(recorded, JSON.stringify({ version: 1, origin: foreign.url }))
        const starting = vscode.commands.executeCommand('tola.startPreview', site.source)
        await gate.next()
        gate.release()
        await within(starting, 'Owned preview startup')
        assert.equal(foreignRequests, 0, 'A prior server cannot answer this startup')
      }, { prepare: gate.prepare })
    } finally {
      gate.release()
      await gate.close()
      await foreign.close()
    }
  })

  await check('Output lookup leaves configuration JSON private', () =>
    withSite(async (site) => {
      const channel = vscode.window.createOutputChannel('Tola output lookup')
      const output: string[] = []
      const recorded = new Proxy(channel, {
        get(target, property) {
          if (property === 'append' || property === 'appendLine') {
            return (text: string) => {
              output.push(text)
              target[property](text)
            }
          }
          const value = Reflect.get(target, property)
          return typeof value === 'function' ? value.bind(target) : value
        },
      })
      const cli = new SiteCli(recorded, (error) => {
        throw error
      })
      const lifetime = cli.register()
      try {
        const folder = vscode.workspace.getWorkspaceFolder(site.source)!
        const selection = await selectSite(folder, site.configuration)
        assert.ok(selection)
        await cli.revealOutput(selection)
        await workbenchText({ contains: 'has not built this site yet', dismiss: true })
        assert.doesNotMatch(output.join(''), /"output"\s*:/)
      } finally {
        lifetime.dispose()
        channel.dispose()
      }
    }))

  await check('Open Page opens the checked route', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        await openPublishedPage('tola.openPage', site.source, opened)
        assert.equal(opened.length, 1)
        assert.equal(opened[0]!.url.pathname, '/preview/')
      })
    ))

  await check(
    'An explicit published fragment opens with its exact encoded ID',
    () =>
      withPreview((opened) =>
        withSite(async (site) => {
          const input = vscode.window.showInputBox
          vscode.window.showInputBox = (() => Promise.resolve('part%2F中文')) as typeof input
          try {
            await vscode.window.showTextDocument(site.source)
            await vscode.commands.executeCommand('tola.openPreviewFragment', site.source)
            assert.equal(opened.length, 1)
            assert.equal(opened[0]!.url.pathname, '/preview/')
            assert.equal(decodeURIComponent(opened[0]!.url.hash.slice(1)), 'part%2F中文')
          } finally {
            vscode.window.showInputBox = input
          }
        }, {
          program:
            '#document("preview/index.html", format: "html")[\n#include "content/page.typ"\n#html.elem("span", attrs: (id: "part%2F中文"))[]\n]\n',
        })
      ),
  )

  await check('A lens data route previews its page', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        await vscode.window.showTextDocument(site.source)
        await vscode.commands.executeCommand('tola.openPreview', site.source, {
          route: '/preview/',
        })
        const page = opened.at(-1)
        assert.ok(page, 'the preview opened no page')
        assert.equal(page.status, 200)
        assert.equal(page.url.pathname, '/preview/')
      })
    ))

  await check('A component opens no preview', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        const component = vscode.Uri.file(path.join(site.root, 'components/nav.typ'))
        await fs.mkdir(path.dirname(component.fsPath), { recursive: true })
        await fs.writeFile(component.fsPath, '= Nav\n')
        await vscode.window.showTextDocument(component)
        await vscode.commands.executeCommand('tola.openPreview', component)
        assert.deepEqual(opened, [])
        assert.match(
          await workbenchText({ contains: 'publishes no page', dismiss: true }),
          /publishes no page/,
        )
      })
    ))

  await check('Stale lenses refuse another route', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        await vscode.window.showTextDocument(site.source)
        const lenses = await vscode.commands.executeCommand<vscode.CodeLens[]>(
          'vscode.executeCodeLensProvider',
          site.source,
        )
        const lens = lenses.find((candidate) => candidate.command?.command === 'tola.openPreview')
        assert.ok(lens?.command)
        await fs.writeFile(
          site.program.fsPath,
          '#document("replacement/index.html", format: "html")[#include "content/page.typ"]\n',
        )
        await vscode.commands.executeCommand(lens.command.command, ...(lens.command.arguments ?? []))
        const refusal = await workbenchText({ error: true, dismiss: true })
        assert.match(refusal, /selected route.*refresh Pages/i)
        assert.deepEqual(opened, [])
      })
    ))

  await check(
    'Site-owned preview opens generated pages',
    () =>
      withPreview((opened) =>
        withSite(async (site) => {
          await vscode.window.showTextDocument(site.plain)
          await vscode.commands.executeCommand('tolaPages.focus')
          assert.match(await workbenchText({ pages: '/generated/', capture: true }), /\/generated\//)
          await vscode.commands.executeCommand(
            'tola.openPreview',
            undefined,
            '/generated/',
            vscode.Uri.file(site.root).toString(),
          )
          assert.equal(opened[0]!.url.pathname, '/generated/')
          assert.equal(opened[0]!.status, 200)
          assert.match(opened[0]!.html, /Root-authored generated page/)
        }, { program: '#document("generated/index.html", format: "html")[Root-authored generated page]\n' })
      ),
  )

  await check('Pages follow the selected configuration root', async () => {
    for (const location of ['inside', 'outside']) {
      const route = `/selected-${location}/`
      await withPreview((_opened, firstOpen) =>
        withSite(async (site) => {
          await vscode.window.showTextDocument(site.plain)
          await vscode.commands.executeCommand('tolaPages.focus')
          await workbenchText({ page: route, capture: true })
          const opened = await within(firstOpen, 'Selected-root page preview')
          assert.equal(opened.url.pathname, route)
          assert.equal(opened.status, 200)
          assert.match(opened.html, /Selected configuration root page/)
        }, {
          prepare: async (site) => {
            const root = location === 'inside'
              ? path.join(site.root, 'nested')
              : path.join(path.dirname(site.root), 'outside selected site')
            await fs.mkdir(path.join(root, 'content'), { recursive: true })
            await fs.writeFile(path.join(root, 'selected.toml'), '[server]\nport = 0\n')
            await fs.writeFile(
              path.join(root, 'site.typ'),
              `#document("selected-${location}/index.html", format: "html")[#include "content/page.typ"]\n`,
            )
            await fs.writeFile(path.join(root, 'content/page.typ'), 'Selected configuration root page.\n')
            const settings = path.join(site.root, '.vscode/settings.json')
            const configured = JSON.parse(await fs.readFile(settings, 'utf8'))
            configured['tola.configPath'] = path.relative(site.root, path.join(root, 'selected.toml'))
            await fs.writeFile(settings, JSON.stringify(configured))
          },
        })
      )
    }
  })

  await check(
    'Start Preview leaves navigation unchanged',
    () =>
      withNotices((notices) =>
        withPreview((opened) =>
          withSite(async (site) => {
            await vscode.window.showTextDocument(site.source)
            await vscode.commands.executeCommand('tola.startPreview', site.source)
            assert.match(await workbenchText({ status: '/preview/' }), /preview/)
            assert.deepEqual(opened, [])
            assert.deepEqual(notices, [])
          })
        )
      ),
  )
  await check(
    'Cancelled preview leaves buffers dirty',
    () =>
      withPreview((opened) =>
        withSite(async (site) => {
          const source = await vscode.workspace.openTextDocument(site.source)
          const unrelated = await vscode.workspace.openTextDocument(site.plain)
          await replace(source, 'Unsaved selected source.\n')
          await replace(unrelated, 'Unsaved unrelated source.\n')
          await vscode.window.showTextDocument(source)
          const preview = vscode.commands.executeCommand('tola.openPreview', source.uri)
          await workbenchText({ button: 'Save and Preview', dismiss: true })
          await preview
          assert.equal(source.isDirty, true)
          assert.equal(unrelated.isDirty, true)
          assert.equal(await fs.readFile(site.source.fsPath, 'utf8'), 'Published editor page.\n')
          assert.equal(await fs.readFile(site.plain.fsPath, 'utf8'), 'Unrelated saved source.\n')
          assert.deepEqual(opened, [])
        })
      ),
  )

  await check(
    'Preview saves cross-workspace dependencies',
    () =>
      withPreview((opened) =>
        withSites(2, async ([site, other]) => {
          assert.ok(site && other)
          const dependency = await vscode.workspace.openTextDocument(
            vscode.Uri.file(path.join(other.root, 'packages/local/shared/0.1.0/lib.typ')),
          )
          const unrelated = await vscode.workspace.openTextDocument(other.source)
          await replace(dependency, '#let message = [Saved cross-workspace dependency.]\n')
          await replace(unrelated, 'Unrelated workspace edit.\n')
          await vscode.window.showTextDocument(site.source)
          const preview = vscode.commands.executeCommand('tola.openPreview', site.source)
          const prompt = await workbenchText({ button: 'Save and Preview' })
          assert.match(prompt, /lib\.typ/)
          assert.doesNotMatch(prompt, /page\.typ/)
          await within(preview, 'Cross-workspace preview')
          assert.equal(dependency.isDirty, false)
          assert.equal(unrelated.isDirty, true)
          assert.equal(await fs.readFile(other.source.fsPath, 'utf8'), 'Published editor page.\n')
          assert.match(opened.at(-1)!.html, /Saved cross-workspace dependency\./)
        }, {
          prepare: async (site) => {
            const packages = path.join(path.dirname(site.root), 'selected site [2]', 'packages')
            const shared = path.join(packages, 'local/shared/0.1.0')
            await fs.mkdir(shared, { recursive: true })
            await fs.writeFile(
              path.join(shared, 'typst.toml'),
              '[package]\nname = "shared"\nversion = "0.1.0"\nentrypoint = "lib.typ"\n',
            )
            await fs.writeFile(path.join(shared, 'lib.typ'), '#let message = [Saved dependency.]\n')
            await fs.writeFile(site.source.fsPath, '#import "@local/shared:0.1.0": message\n#message\n')
            const settings = path.join(site.root, '.vscode/settings.json')
            const configured = JSON.parse(await fs.readFile(settings, 'utf8'))
            configured['tola.packagePath'] = packages
            await fs.writeFile(settings, JSON.stringify(configured))
          },
        })
      ),
  )

  await check('Preview saves discovered package changes', async () => {
    const previous = process.env.TYPST_PACKAGE_CACHE_PATH
    let packageUri: vscode.Uri
    try {
      await withPreview((opened) =>
        withSite(async (site) => {
          const source = await vscode.workspace.openTextDocument(site.source)
          const target = await vscode.commands.executeCommand<vscode.Location[]>(
            'vscode.executeDefinitionProvider',
            site.source,
            source.positionAt(source.getText().lastIndexOf('message')),
          )
          assert.equal(target[0]?.uri.toString(), packageUri.toString(), 'the site reads this cache package')
          const dependency = await vscode.workspace.openTextDocument(packageUri)
          await replace(dependency, '#let message = [Unsaved package change.]\n')
          await vscode.window.showTextDocument(site.source)
          const preview = vscode.commands.executeCommand('tola.openPreview', site.source)
          const prompt = await workbenchText({ button: 'Save and Preview' })
          assert.match(prompt, /lib\.typ/)
          await within(preview, 'Discovered package preview')
          assert.equal(dependency.isDirty, false)
          assert.match(opened.at(-1)!.html, /Unsaved package change\./)
          await vscode.window.showTextDocument(dependency)
          await within(
            vscode.commands.executeCommand('tola.openPreview', dependency.uri),
            'Package-source preview',
          )
          assert.equal(opened.length, 2, 'the package source has one site to preview it')
          await within(
            vscode.commands.executeCommand('tola.openPage', dependency.uri),
            'Package-source page',
          )
          assert.equal(opened.length, 3)
          const siteKey = vscode.workspace.getWorkspaceFolder(site.source)!.uri.toString()
          await vscode.commands.executeCommand('tola.openPreview', dependency.uri, '/preview/', siteKey)
          assert.equal(opened.length, 4)
          assert.match(await workbenchText({ status: '/preview/' }), /preview/)
          const clipboard = await vscode.env.clipboard.readText()
          try {
            await vscode.commands.executeCommand('tola.copyRoute')
            assert.equal(await vscode.env.clipboard.readText(), '/preview/')
          } finally {
            await vscode.env.clipboard.writeText(clipboard)
          }
        }, {
          content: '#import "@preview/preview-owned:0.1.0": message\n#message\n',
          prepare: async (site) => {
            const packageRoot = path.join(path.dirname(site.root), 'discovered-cache')
            const library = path.join(packageRoot, 'preview/preview-owned/0.1.0')
            await fs.mkdir(library, { recursive: true })
            await fs.writeFile(
              path.join(library, 'typst.toml'),
              '[package]\nname = "preview-owned"\nversion = "0.1.0"\nentrypoint = "lib.typ"\n',
            )
            packageUri = vscode.Uri.file(path.join(library, 'lib.typ'))
            await fs.writeFile(packageUri.fsPath, '#let message = [Saved package value.]\n')
            process.env.TYPST_PACKAGE_CACHE_PATH = packageRoot
          },
        })
      )
    } finally {
      if (previous === undefined) delete process.env.TYPST_PACKAGE_CACHE_PATH
      else process.env.TYPST_PACKAGE_CACHE_PATH = previous
    }
  })

  await check('Shared package cache has no preview owner', async () => {
    const previous = process.env.TYPST_PACKAGE_CACHE_PATH
    try {
      await withSites(2, async (sites) => {
        for (const site of sites) {
          await vscode.window.showTextDocument(site.source)
          await workbenchText({ status: '(?<!not )checked' })
        }
        const source = vscode.Uri.file(
          path.join(process.env.TYPST_PACKAGE_CACHE_PATH!, 'preview/shared/0.1.0/lib.typ'),
        )
        await vscode.window.showTextDocument(source)
        await vscode.commands.executeCommand('tola.openPreview', source)
        assert.match(await workbenchText({ error: true, dismiss: true }), /no site answers/)
      }, {
        prepare: async (site) => {
          const cache = path.join(path.dirname(site.root), 'shared-cache')
          const library = path.join(cache, 'preview/shared/0.1.0')
          await fs.mkdir(library, { recursive: true })
          await fs.writeFile(path.join(library, 'lib.typ'), '#let value = 1\n')
          process.env.TYPST_PACKAGE_CACHE_PATH = cache
        },
      })
    } finally {
      if (previous === undefined) delete process.env.TYPST_PACKAGE_CACHE_PATH
      else process.env.TYPST_PACKAGE_CACHE_PATH = previous
    }
  })

  await check('Preview includes a site-local package source', () =>
    withPreview((opened) => {
      let packageUri: vscode.Uri
      return withSite(async (site) => {
        const source = await vscode.workspace.openTextDocument(site.source)
        const target = await vscode.commands.executeCommand<vscode.Location[]>(
          'vscode.executeDefinitionProvider',
          site.source,
          source.positionAt(source.getText().lastIndexOf('message')),
        )
        assert.equal(target[0]?.uri.toString(), packageUri.toString(), 'the site reads its local package')
        await vscode.window.showTextDocument(packageUri)
        await vscode.commands.executeCommand('tola.openPreview', packageUri)
        assert.equal(opened.length, 1)
        assert.equal(opened[0]!.url.pathname, '/preview/')
        const address = new URL('/_tola/preview', opened[0]!.url)
        const routesFor = async (source: vscode.Uri) => {
          address.searchParams.set('source', source.toString())
          const reply = await fetch(address, { headers: { 'X-Tola-Preview': '1' } })
          return { status: reply.status, routes: (await reply.json() as { routes: unknown[] }).routes }
        }
        const unused = vscode.Uri.file(path.join(site.root, 'unused.typ'))
        await fs.writeFile(unused.fsPath, 'Unused.\n')
        assert.deepEqual(await routesFor(unused), { status: 200, routes: [] })
        assert.equal(
          (await routesFor(vscode.Uri.file(path.join(path.dirname(site.root), 'foreign.typ')))).status,
          400,
        )
      }, {
        content: '#import "@local/local-preview:0.1.0": message\n#message\n',
        prepare: async (site) => {
          const packages = path.join(site.root, 'packages')
          const library = path.join(packages, 'local/local-preview/0.1.0')
          await fs.mkdir(library, { recursive: true })
          await fs.writeFile(
            path.join(library, 'typst.toml'),
            '[package]\nname = "local-preview"\nversion = "0.1.0"\nentrypoint = "lib.typ"\n',
          )
          packageUri = vscode.Uri.file(path.join(library, 'lib.typ'))
          await fs.writeFile(packageUri.fsPath, '#let message = [Local package content.]\n')
          const settings = path.join(site.root, '.vscode/settings.json')
          const configured = JSON.parse(await fs.readFile(settings, 'utf8'))
          configured['tola.packagePath'] = packages
          await fs.writeFile(settings, JSON.stringify(configured))
        },
      })
    }))

  await check('Saved preview waits for publication', async () => {
    let holding = false
    const gate = await publicationGate((html) =>
      holding &&
      html.includes('Saved selected root.') && html.includes('Saved selected dependency.')
    )
    try {
      await withPreview((opened) =>
        withSite(async (site) => {
          await openPublishedPage('tola.openPreview', site.source, opened)
          assert.equal(opened.length, 1)
          const program = await vscode.workspace.openTextDocument(site.program)
          const source = await vscode.workspace.openTextDocument(site.source)
          const unrelated = await vscode.workspace.openTextDocument(site.plain)
          await replace(
            program,
            '#document("new/index.html", format: "html")[Saved selected root. #include "content/page.typ"]\n',
          )
          await replace(source, 'Saved selected dependency.\n')
          await replace(unrelated, 'Unrelated edit must remain unsaved.\n')
          await vscode.window.showTextDocument(source)
          holding = true
          const preview = vscode.commands.executeCommand('tola.openPreview', site.source)
          const prompt = await workbenchText({ button: 'Save and Preview' })
          assert.match(prompt, /site\.typ/)
          assert.match(prompt, /page\.typ/)
          assert.doesNotMatch(prompt, /ordinary\.typ/)
          const held = await gate.next()
          assert.match(held.html, /Saved selected root\./)
          assert.equal(opened.length, 1, 'The unpublished candidate must not be opened')
          gate.release()
          await within(preview, 'Saved preview')
          assert.equal(program.isDirty, false)
          assert.equal(source.isDirty, false)
          assert.equal(unrelated.isDirty, true)
          assert.equal(await fs.readFile(site.plain.fsPath, 'utf8'), 'Unrelated saved source.\n')
          assert.equal(opened.length, 2)
          assert.equal(opened[1]!.url.pathname, '/new/')
          assert.equal(opened[1]!.status, 200)
          assert.match(opened[1]!.html, /Saved selected dependency\./)
          assert.match(opened[1]!.html, /Saved selected root\./)
        }, {
          program: '#document("old/index.html", format: "html")[#include "content/page.typ"]\n',
          prepare: gate.prepare,
        })
      )
    } finally {
      gate.release()
      await gate.close()
    }
  })

  await check('Failed preview keeps published bytes', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        const source = await vscode.workspace.openTextDocument(site.source)
        const previous = await openPublishedPage('tola.openPreview', site.source, opened)
        assert.equal(opened.length, 1)
        await replace(source, '#panic("Preview regression failed")\n')
        const preview = vscode.commands.executeCommand('tola.openPreview', site.source)
        await workbenchText({ button: 'Save and Preview' })
        await within(preview, 'Failed preview')
        await workbenchText({ error: true, dismiss: true })
        assert.equal(source.isDirty, false)
        assert.equal(opened.length, 1)
        const response = await fetch(previous.url, { signal: AbortSignal.timeout(15_000) })
        assert.equal(response.status, 200)
        assert.match(await response.text(), /Published editor page\./)
      })
    ))

  await check(
    'Mounted lenses open their published URL',
    () =>
      withPreview((opened) =>
        withSite(async (site) => {
          await vscode.window.showTextDocument(site.source)
          const lenses = await vscode.commands.executeCommand<vscode.CodeLens[]>(
            'vscode.executeCodeLensProvider',
            site.source,
          )
          const lens = lenses.find((candidate) => candidate.command?.command === 'tola.openPreview')
          assert.ok(lens?.command, 'The published source has no preview lens')
          await vscode.commands.executeCommand(lens.command.command, ...(lens.command.arguments ?? []))
          assert.equal(opened.length, 1)
          assert.equal(opened[0]!.url.pathname, '/docs/preview/')
          assert.equal(opened[0]!.status, 200)
          assert.match(opened[0]!.html, /Published editor page\./)
        }, { configuration: '[site]\ntitle = "Mounted editor site"\nbase-path = "/docs/"\n' })
      ),
  )

  await check(
    'Copied routes ignore unsaved edits',
    () =>
      withNotices((notices) =>
        withPreview((opened) =>
          withSite(async (site) => {
            await openPublishedPage('tola.openPreview', site.source, opened)
            const program = await vscode.workspace.openTextDocument(site.program)
            const saved = program.getText()
            await replace(
              program,
              '#document("unsaved/index.html", format: "html")[#include "content/page.typ"]\n',
            )
            await vscode.window.showTextDocument(site.source)
            const clipboard = await vscode.env.clipboard.readText()
            try {
              await vscode.commands.executeCommand('tola.copyRoute')
              assert.equal(await vscode.env.clipboard.readText(), '/preview/')
              assert.equal(program.isDirty, true)
              assert.equal(await fs.readFile(site.program.fsPath, 'utf8'), saved)
              assert.equal(opened.length, 1)
              assert.deepEqual(notices, [])
            } finally {
              await vscode.env.clipboard.writeText(clipboard)
            }
          })
        )
      ),
  )

  await check('Config switches replace the preview', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        await openPublishedPage('tola.openPreview', site.source, opened)
        await fs.writeFile(
          path.join(site.root, 'alternate.toml'),
          '[site]\nbase-path = "/alternate/"\n\n[server]\nport = 0\n',
        )
        await vscode.workspace.getConfiguration('tola', site.source)
          .update('configPath', 'alternate.toml', vscode.ConfigurationTarget.WorkspaceFolder)
        await vscode.commands.executeCommand('tola.restart')
        await openPublishedPage('tola.openPreview', site.source, opened)
        assert.equal(opened.length, 2)
        assert.equal(opened[1]!.url.pathname, '/alternate/preview/')
      })
    ))

  await check('Concurrent sites own their startup', async () => {
    const gate = await publicationGate(() => true)
    try {
      await withPreview((opened) =>
        withSite((blocked) =>
          withSite(async (independent) => {
            await vscode.window.showTextDocument(blocked.source)
            const starting = vscode.commands.executeCommand('tola.openPreview', blocked.source)
            await gate.next()
            await vscode.window.showTextDocument(independent.source)
            assert.doesNotMatch(await workbenchText({ status: 'Start Preview' }), /publishing/)
            await within(
              vscode.commands.executeCommand('tola.openPreview', independent.source),
              'Independent preview',
            )
            assert.equal(opened.length, 1)
            assert.match(opened[0]!.html, /Independent site publication\./)
            gate.release()
            await within(starting, 'Blocked preview')
            assert.equal(opened.length, 2)
            assert.notEqual(opened[0]!.url.origin, opened[1]!.url.origin)
            assert.match(opened[1]!.html, /Blocked site publication\./)
          }, { content: 'Independent site publication.\n' }), {
          content: 'Blocked site publication.\n',
          prepare: gate.prepare,
        })
      )
    } finally {
      gate.release()
      await gate.close()
    }
  })

  await check('Restart stops pending preview children', async () => {
    const gate = await publicationGate(() => true)
    try {
      await withPreview((opened) =>
        withSite(async (site) => {
          await vscode.window.showTextDocument(site.source)
          const starting = vscode.commands.executeCommand('tola.openPreview', site.source)
          const held = await gate.next()
          await vscode.commands.executeCommand('tola.restart')
          await within(starting, 'Cancelled preview startup')
          await within(held.closed, 'Cancelled publication hook')
          assert.deepEqual(opened, [])
          assertStopped(held.devPid)
          assertStopped(held.hookPid)
        }, { prepare: gate.prepare })
      )
    } finally {
      gate.release()
      await gate.close()
    }
  })
}
