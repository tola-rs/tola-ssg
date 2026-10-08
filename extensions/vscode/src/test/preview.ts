import * as assert from 'node:assert/strict'
import * as fs from 'node:fs/promises'
import type { ServerResponse } from 'node:http'
import * as path from 'node:path'
import * as vscode from 'vscode'
import { startGate } from '../../../../e2e/support/gate.ts'
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

type HeldPublication = { html: string; devPid: number; hookPid: number; closed: Promise<void> }

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
  await check('Open Page opens the checked route', () =>
    withPreview((opened) =>
      withSite(async (site) => {
        await openPublishedPage('tola.openPage', site.source, opened)
        assert.equal(opened.length, 1)
        assert.equal(opened[0]!.url.pathname, '/preview/')
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
      withPreview((opened) =>
        withSite(async (site) => {
          await vscode.window.showTextDocument(site.source)
          await vscode.commands.executeCommand('tola.startPreview', site.source)
          assert.match(
            await workbenchText({ contains: `Tola published ${site.name}`, dismiss: true }),
            /http:/,
          )
          assert.deepEqual(opened, [])
        })
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

  await check('Copied routes ignore unsaved edits', () =>
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
        } finally {
          await vscode.env.clipboard.writeText(clipboard)
        }
      })
    ))

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
