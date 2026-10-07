import * as assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { existsSync, watch } from 'node:fs'
import * as fs from 'node:fs/promises'
import type { ServerResponse } from 'node:http'
import * as os from 'node:os'
import * as path from 'node:path'
import { promisify } from 'node:util'
import * as vscode from 'vscode'
import { startGate } from '../../../../e2e/support/gate.ts'

const channelLog: string[] = []
const channelLines = new vscode.EventEmitter<string>()
let recording = false

/**
 * Keep every line the extension writes to an output channel, so a case reads what the author sees.
 *
 * Installed before the extension activates, since that is when it creates the channel, and once
 * per window: the channels it later asks for are the same ones.
 */
export function recordOutputChannels(): void {
  if (recording) return
  recording = true
  const create = vscode.window.createOutputChannel
  vscode.window.createOutputChannel = ((name: string) =>
    new Proxy(create(name), {
      get: (channel, property) => {
        if (property === 'append' || property === 'appendLine' || property === 'replace') {
          return (value: string) => {
            channelLog.push(value)
            channelLines.fire(value)
            ;(channel[property] as (value: string) => void)(value)
          }
        }
        const value = Reflect.get(channel, property)
        return typeof value === 'function' ? value.bind(channel) : value
      },
    })) as typeof vscode.window.createOutputChannel
}

/** Every line the extension has written to an output channel, in the order it wrote them. */
export function outputLog(): string {
  return channelLog.join('\n')
}

/** Wait for a line the extension writes to hold `text`, so a case reads what the author reads. */
export async function waitForOutput(text: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const seen = (line: string) => {
      if (!line.includes(text)) return
      clearTimeout(deadline)
      arrival.dispose()
      resolve()
    }
    const arrival = channelLines.event(seen)
    const deadline = setTimeout(() => {
      arrival.dispose()
      reject(new Error(`The output channel never held ${text}`))
    }, 15_000)
    seen(outputLog())
  })
}

export type EditorSite = {
  name: string
  root: string
  plain: vscode.Uri
  configuration: vscode.Uri
  source: vscode.Uri
  program: vscode.Uri
}

type SiteOptions = {
  program?: string
  content?: string
  configuration?: string
  documents?: boolean
  prepare?: (site: EditorSite) => Promise<void>
}

export function recordSiteSurfaces(): {
  siteSelected(): boolean | undefined
  pages(): Promise<vscode.TreeItem[]>
} {
  let selected: boolean | undefined
  let pages: vscode.TreeDataProvider<vscode.TreeItem> | undefined
  const execute = vscode.commands.executeCommand
  vscode.commands.executeCommand = ((command: string, ...args: unknown[]) => {
    if (command === 'setContext' && args[0] === 'tola.site') selected = args[1] as boolean
    return execute(command, ...args)
  }) as typeof vscode.commands.executeCommand
  const create = vscode.window.createTreeView
  vscode.window.createTreeView = ((id: string, options: vscode.TreeViewOptions<vscode.TreeItem>) => {
    if (id === 'tolaPages') pages = options.treeDataProvider
    return create(id, options)
  }) as typeof vscode.window.createTreeView
  return {
    siteSelected: () => selected,
    pages: async () => {
      assert.ok(pages, 'The extension did not register its pages view')
      return await pages.getChildren() ?? []
    },
  }
}

export async function check(name: string, assertion: () => Promise<void>): Promise<void> {
  const filter = process.env.TOLA_TEST_FILTER
  if (filter && !new RegExp(filter).test(name)) return
  console.log(`- ${name}`)
  try {
    await assertion()
  } catch (error) {
    throw new Error(`${name}: ${error instanceof Error ? error.message : String(error)}`, { cause: error })
  }
}

async function changeFolders(remove: vscode.Uri[], add: vscode.Uri[]): Promise<void> {
  const previous = vscode.workspace.workspaceFolders ?? []
  const removed = new Set(remove.map((uri) => uri.toString()))
  const kept = previous.filter((folder) => !removed.has(folder.uri.toString()))
  await new Promise<void>((resolve, reject) => {
    const changed = vscode.workspace.onDidChangeWorkspaceFolders(() => {
      clearTimeout(deadline)
      changed.dispose()
      resolve()
    })
    const deadline = setTimeout(() => {
      changed.dispose()
      reject(new Error('Workspace folders did not change'))
    }, 15_000)
    if (
      !vscode.workspace.updateWorkspaceFolders(
        1,
        previous.length - 1,
        ...kept.slice(1).map((folder) => ({ uri: folder.uri })),
        ...add.map((uri) => ({ uri })),
      )
    ) {
      clearTimeout(deadline)
      changed.dispose()
      reject(new Error('VS Code refused the workspace folders'))
    }
  })
}

/** The sites one check runs against, each a workspace folder with its own configuration and one source. */
export async function withSites(
  count: number,
  assertion: (sites: EditorSite[]) => Promise<void>,
  options: SiteOptions = {},
): Promise<void> {
  // Every path a case hands the editor is spelled as the OS resolves it, the same way the extension
  // spells the configuration it resolves: a case directory reached through a symlink (`TMPDIR` is
  // `/var/...` on macOS, resolving to `/private/var/...`) would otherwise name its own files by a
  // path no site answers for.
  const temporary = await fs.realpath(await fs.mkdtemp(path.join(process.env.TOLA_TEST_ROOT!, 'case-')))
  const plainRoot = path.join(temporary, 'ordinary')
  const plain = vscode.Uri.file(path.join(plainRoot, 'ordinary.typ'))
  const roots = Array.from(
    { length: count },
    (_, index) => path.join(temporary, index === 0 ? 'selected site [one]' : `selected site [${index + 1}]`),
  )
  const sites: EditorSite[] = roots.map((root, index) => ({
    name: path.basename(root),
    root,
    plain,
    // The first site reaches its configuration through a symlink, so both spellings a setting may
    // name it by are covered.
    configuration: vscode.Uri.file(
      options.documents
        ? path.join(root, 'tola.toml')
        : index === 0
        ? path.join(temporary, 'configuration-source.toml')
        : path.join(root, 'selected config.toml'),
    ),
    source: vscode.Uri.file(path.join(root, 'content/page.typ')),
    program: vscode.Uri.file(path.join(root, 'site.typ')),
  }))
  let added = false
  try {
    await fs.mkdir(path.join(plainRoot, '.vscode'), { recursive: true })
    await fs.writeFile(plain.fsPath, 'Unrelated saved source.\n')
    await fs.writeFile(
      path.join(plainRoot, '.vscode/settings.json'),
      JSON.stringify({
        'tola.serverPath': '/must-not-start-in-non-tola-workspace',
        'tola.enabled': false,
      }),
    )
    for (const site of sites) {
      const selectedConfig = path.join(site.root, 'selected config.toml')
      await fs.mkdir(path.join(site.root, '.vscode'), { recursive: true })
      await fs.mkdir(path.join(site.root, 'content'))
      if (!options.documents) {
        await fs.writeFile(selectedConfig, '')
        if (site.configuration.fsPath !== selectedConfig) {
          await fs.rm(selectedConfig)
          await fs.symlink(site.configuration.fsPath, selectedConfig)
        }
        await fs.writeFile(
          site.configuration.fsPath,
          (options.configuration ?? '[site]\ntitle = "Editor contract"\n') + '\n[server]\nport = 0\n',
        )
        await fs.writeFile(path.join(site.root, 'tola.toml'), 'invalid TOML: the selected config must win')
      }
      await fs.writeFile(
        site.program.fsPath,
        options.program ?? '#document("preview/index.html", format: "html")[#include "content/page.typ"]\n',
      )
      await fs.writeFile(site.source.fsPath, options.content ?? 'Published editor page.\n')
      await fs.writeFile(
        path.join(site.root, '.vscode/settings.json'),
        JSON.stringify({
          'tola.serverPath': process.env.TOLA_TEST_BINARY,
          'tola.configPath': options.documents ? 'tola.toml' : 'selected config.toml',
        }),
      )
    }
    const [firstSite] = sites
    assert.ok(firstSite, 'A case runs against at least one site')
    await options.prepare?.(firstSite)
    await changeFolders([], [...sites.map((site) => vscode.Uri.file(site.root)), vscode.Uri.file(plainRoot)])
    added = true
    await vscode.commands.executeCommand('tola.restart')
    await assertion(sites)
  } finally {
    if (added) {
      for (const document of vscode.workspace.textDocuments) {
        if (
          document.isDirty && document.uri.scheme === 'file' &&
          document.uri.fsPath.startsWith(temporary + path.sep)
        ) {
          await vscode.window.showTextDocument(document)
          await vscode.commands.executeCommand('workbench.action.files.revert')
        }
      }
      await changeFolders(
        [...sites.map((site) => vscode.Uri.file(site.root)), vscode.Uri.file(plainRoot)],
        [],
      )
      await vscode.commands.executeCommand('tola.restart')
      await vscode.commands.executeCommand('notifications.clearAll')
      await vscode.commands.executeCommand('workbench.action.closeAllEditors')
    }
    await fs.rm(temporary, { recursive: true, force: true })
  }
}

export async function withSite(
  assertion: (site: EditorSite) => Promise<void>,
  options: SiteOptions = {},
): Promise<void> {
  await withSites(1, async ([site]) => {
    assert.ok(site, 'withSite runs against one site')
    await assertion(site)
  }, options)
}

/** Disable one site without starting its client again. */
export async function disable(site: EditorSite): Promise<void> {
  await vscode.workspace.getConfiguration('tola', site.source)
    .update('enabled', false, vscode.ConfigurationTarget.WorkspaceFolder)
  await vscode.commands.executeCommand('tola.restart')
}

export async function replace(document: vscode.TextDocument, text: string): Promise<void> {
  const edit = new vscode.WorkspaceEdit()
  edit.replace(
    document.uri,
    new vscode.Range(document.positionAt(0), document.positionAt(document.getText().length)),
    text,
  )
  assert.equal(await vscode.workspace.applyEdit(edit), true)
}

export async function openMarked(
  uri: vscode.Uri,
  marked: string,
): Promise<{ document: vscode.TextDocument; position: vscode.Position }> {
  const offset = marked.indexOf('|')
  assert.notEqual(offset, -1, 'The source needs a cursor marker')
  const document = await vscode.workspace.openTextDocument(uri)
  await replace(document, marked.slice(0, offset) + marked.slice(offset + 1))
  await vscode.window.showTextDocument(document)
  return { document, position: document.positionAt(offset) }
}

/** The documents a definition at one position opens, as the editor's own URIs. */
export async function definitionTargets(
  document: vscode.TextDocument,
  position: vscode.Position,
): Promise<vscode.Uri[]> {
  const definitions = await vscode.commands.executeCommand<(vscode.Location | vscode.LocationLink)[]>(
    'vscode.executeDefinitionProvider',
    document.uri,
    position,
  ) ?? []
  return definitions.map((target) => 'targetUri' in target ? target.targetUri : target.uri)
}

/** The outline a site answers for one source: it proves which server holds the document open. */
export async function symbols(
  uri: vscode.Uri,
): Promise<Array<vscode.DocumentSymbol | vscode.SymbolInformation>> {
  return await vscode.commands.executeCommand<Array<vscode.DocumentSymbol | vscode.SymbolInformation>>(
    'vscode.executeDocumentSymbolProvider',
    uri,
  ) ?? []
}

/** Answer requests while the author's site choice is scripted. */
export async function withSiteChoice(
  choice: (labels: string[]) => string | undefined,
  assertion: (offered: string[][]) => Promise<void>,
): Promise<void> {
  const original = vscode.window.showQuickPick
  const offered: string[][] = []
  vscode.window.showQuickPick = ((items: readonly vscode.QuickPickItem[]) => {
    const labels = items.map((item) => item.label)
    offered.push(labels)
    const label = choice(labels)
    return items.find((item) => item.label === label)
  }) as unknown as typeof vscode.window.showQuickPick
  try {
    await assertion(offered)
  } finally {
    vscode.window.showQuickPick = original
  }
}

/** Wait for one document to leave the workspace, so a transition's own event is observed. */
export async function waitForClose(uri: vscode.Uri): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const closed = vscode.workspace.onDidCloseTextDocument((document) => {
      if (document.uri.toString() !== uri.toString()) return
      clearTimeout(deadline)
      closed.dispose()
      resolve()
    })
    const deadline = setTimeout(() => {
      closed.dispose()
      reject(new Error(`Document close timed out for ${uri.toString()}`))
    }, 15_000)
  })
}

export type BlockedStart = {
  /** Let the held service run. */
  release(): Promise<void>
  /** Release the service and remove what held it. */
  close(): Promise<void>
}

/**
 * Hold one site's next language service before it runs, and report when its process is reached.
 *
 * The site's `tola.serverPath` becomes a script that reports in and then waits for the gate. This
 * resolves after the previous client is stopped and the new process is held, so a request made then
 * meets a service that is starting and only the client that comes up can answer it.
 */
export async function blockServerStart(site: EditorSite): Promise<BlockedStart> {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'tola-start-gate-'))
  const marker = path.join(directory, 'started')
  const gate = path.join(directory, 'gate')
  const script = path.join(directory, 'service.sh')
  await promisify(execFile)('mkfifo', [gate])
  await fs.writeFile(
    script,
    `#!/bin/sh\n: > ${quoted(marker)}\nexec 3< ${quoted(gate)}\nexec ${
      quoted(process.env.TOLA_TEST_BINARY ?? '')
    } "$@"\n`,
    { mode: 0o755 },
  )
  const reached = waitForFile(directory, marker)
  const setting = () =>
    vscode.workspace.getConfiguration('tola', site.source).get<string>('serverPath')
  await vscode.workspace.getConfiguration('tola', site.source)
    .update('serverPath', script, vscode.ConfigurationTarget.WorkspaceFolder)
  assert.equal(
    setting(),
    script,
    'The workspace folder did not keep the service path this test wrote',
  )
  await reached
  let released: Promise<void> | undefined
  const release = () => released ??= fs.open(gate, 'w').then((handle) => handle.close())
  return {
    release,
    close: async () => {
      await release()
      await fs.rm(directory, { recursive: true, force: true })
    },
  }
}

/** A path as one shell word, so a generated script reads it literally. */
function quoted(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`
}

/** Wait for one file to appear, so a process that wrote it there has been reached. */
async function waitForFile(directory: string, file: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const seen = () => {
      if (!existsSync(file)) return
      watcher.close()
      clearTimeout(deadline)
      resolve()
    }
    const watcher = watch(directory, seen)
    const deadline = setTimeout(() => {
      watcher.close()
      reject(new Error(`The held service did not start for ${file}`))
    }, 60_000)
    seen()
  })
}

export type RecordedService = {
  /** The bytes the client has sent this service, as they arrived on its own stream. */
  messages(): Promise<string>
  /** How many times this service has been started to run `verb`. */
  starts(verb: string): Promise<number>
  /** The process one start of this service left behind, for `verb`; undefined before it starts. */
  servicePid(verb: string): Promise<number | undefined>
  /** Resolve once the client has sent a message holding `text`, at or after byte `from`. */
  waitForMessage(text: string, from?: number): Promise<void>
}

/**
 * Make one site's service a proxy that records what the client sends it.
 *
 * The site's `tola.serverPath` becomes a script that runs the proxy, which forwards the protocol
 * to the real service byte for byte and appends the client's own stream, and one line per start,
 * to its own log. Resolves once the proxy has started, so the log already holds what the client
 * sent while it connected.
 */
export async function recordServiceMessages(site: EditorSite): Promise<RecordedService> {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'tola-service-log-'))
  const messages = path.join(directory, 'messages.log')
  const starts = path.join(directory, 'starts.log')
  const proxy = path.join(directory, 'proxy.cjs')
  const runner = path.join(directory, 'service.sh')
  await fs.writeFile(proxy, proxySource(messages, starts, process.env.TOLA_TEST_BINARY ?? ''))
  await fs.writeFile(runner, `#!/bin/sh\nexec "\${TOLA_TEST_NODE:-node}" ${quoted(proxy)} "$@"\n`, {
    mode: 0o755,
  })
  await vscode.workspace.getConfiguration('tola', site.source)
    .update('serverPath', runner, vscode.ConfigurationTarget.WorkspaceFolder)
  await waitForFile(directory, starts)
  return {
    messages: () => fs.readFile(messages, 'utf8').catch(() => ''),
    starts: async (verb) => (await readStarts(starts)).filter((entry) => entry.args.includes(verb)).length,
    servicePid: async (verb) =>
      (await readStarts(starts)).filter((entry) => entry.args.includes(verb)).at(-1)
        ?.service,
    waitForMessage: (text, from = 0) =>
      waitForLog(directory, messages, (log) => log.slice(from).includes(text), text),
  }
}

/** A client request held before it reaches the service, with the release that lets it through. */
export type HeldRequest = {
  /** Resolves once the client sent the held method, with the request still unanswered. */
  reached: Promise<void>
  /** Let the held request reach the service. */
  release(): void
  /** Release the request and remove what held it. */
  close(): Promise<void>
}

export async function blockRequest(
  site: EditorSite,
  method: string,
  checkMode?: 'onSave',
): Promise<HeldRequest> {
  let response: ServerResponse | undefined
  let observe!: () => void
  const reached = new Promise<void>((resolve) => {
    observe = resolve
  })
  const gate = await startGate((_request, held) => {
    response = held
    observe()
  })
  const proxy = path.join(site.root, 'held-request.cjs')
  const runner = path.join(site.root, 'held-request.sh')
  await fs.writeFile(
    proxy,
    `
const { spawn } = require('node:child_process');
const service = spawn(${
      JSON.stringify(process.env.TOLA_TEST_BINARY)
    }, process.argv.slice(2), { stdio: ['pipe', 'inherit', 'inherit'] });
let pending = Buffer.alloc(0);
let held = false;
const settings = ${
      JSON.stringify(
        checkMode
          ? {
            jsonrpc: '2.0',
            method: 'workspace/didChangeConfiguration',
            params: { settings: { checkMode } },
          }
          : null,
      )
    };
process.stdin.on('data', chunk => {
  pending = Buffer.concat([pending, chunk]);
  while (true) {
    const header = pending.indexOf('\\r\\n\\r\\n');
    if (header < 0) return;
    const length = Number(/Content-Length: (\\d+)/i.exec(pending.subarray(0, header).toString())[1]);
    const end = header + 4 + length;
    if (pending.length < end) return;
    const frame = pending.subarray(0, end);
    const message = JSON.parse(pending.subarray(header + 4, end).toString());
    pending = pending.subarray(end);
    if (!held && message.method === ${JSON.stringify(method)}) {
      held = true;
      fetch(${JSON.stringify(gate.url)}, { method: 'POST' })
        .then(response => response.text()).then(() => service.stdin.write(frame));
    } else service.stdin.write(frame);
    if (settings && message.method === 'initialized') {
      const body = Buffer.from(JSON.stringify(settings));
      service.stdin.write(Buffer.concat([Buffer.from('Content-Length: ' + body.length + '\\r\\n\\r\\n'), body]));
    }
  }
});
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, () => service.kill(signal));
service.on('exit', (code, signal) => { if (signal) process.kill(process.pid, signal); else process.exit(code ?? 0); });
`,
  )
  await fs.writeFile(runner, `#!/bin/sh\nexec "\${TOLA_TEST_NODE:-node}" ${quoted(proxy)} "$@"\n`, {
    mode: 0o755,
  })
  await vscode.workspace.getConfiguration('tola', site.source)
    .update('serverPath', runner, vscode.ConfigurationTarget.WorkspaceFolder)
  await vscode.commands.executeCommand('tola.restart')
  return {
    reached,
    release: () => response?.end('continue'),
    close: async () => {
      response?.end('continue')
      await gate.close()
    },
  }
}

/** The proxy a recorded service runs: the real service, with the client's own stream kept. */
function proxySource(messages: string, starts: string, executable: string): string {
  return `const { spawn } = require("node:child_process");
const { appendFileSync } = require("node:fs");
const { Transform } = require("node:stream");
const args = process.argv.slice(2);
const service = spawn(${JSON.stringify(executable)}, args, { stdio: ["pipe", "inherit", "inherit"] });
appendFileSync(${JSON.stringify(starts)}, JSON.stringify({ args, service: service.pid }) + "\\n");
const keep = new Transform({
  transform(chunk, _encoding, complete) { appendFileSync(${
    JSON.stringify(messages)
  }, chunk); complete(null, chunk); },
});
process.stdin.pipe(keep).pipe(service.stdin);
// A caller ends the service it spawned, which is this proxy: the real one has to go with it.
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => service.kill(signal));
}
service.on("exit", (code, signal) => {
  // The caller watches this proxy as its service, so the real ending is the one it must see.
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 0);
});
`
}

/** Wait for one log file to hold what `accept` reads, so the process that writes it was reached. */
async function waitForLog(
  directory: string,
  file: string,
  accept: (log: string) => boolean,
  wanted: string,
): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const seen = () => {
      void fs.readFile(file, 'utf8').catch(() => '').then((log) => {
        if (!accept(log)) return
        watcher.close()
        clearTimeout(deadline)
        resolve()
      })
    }
    const watcher = watch(directory, seen)
    const deadline = setTimeout(() => {
      watcher.close()
      reject(new Error(`The service was never sent ${wanted}`))
    }, 15_000)
    seen()
  })
}

/** One line per service start: the arguments it ran with and the process the real service got. */
async function readStarts(starts: string): Promise<Array<{ args: string[]; service?: number }>> {
  const log = await fs.readFile(starts, 'utf8').catch(() => '')
  return log.split('\n').filter((line) => line.length > 0).map((line) => JSON.parse(line))
}

export async function typstDocument(uri: vscode.Uri): Promise<vscode.TextDocument> {
  await vscode.workspace.openTextDocument(uri)
  return new Promise<vscode.TextDocument>((resolve, reject) => {
    const observe = () => {
      const document = vscode.workspace.textDocuments.find((candidate) =>
        candidate.uri.toString() === uri.toString() && candidate.languageId === 'typst'
      )
      if (!document) return
      clearTimeout(deadline)
      opened.dispose()
      resolve(document)
    }
    const opened = vscode.workspace.onDidOpenTextDocument(observe)
    const deadline = setTimeout(() => {
      opened.dispose()
      reject(new Error(`Typst language attribution timed out for ${uri.toString()}`))
    }, 15_000)
    observe()
  })
}

export async function waitForDiagnostics(
  uri: vscode.Uri,
  accept: (diagnostics: readonly vscode.Diagnostic[]) => boolean,
): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const check = () => {
      if (!accept(vscode.languages.getDiagnostics(uri))) return
      clearTimeout(deadline)
      changed.dispose()
      resolve()
    }
    const changed = vscode.languages.onDidChangeDiagnostics(check)
    const deadline = setTimeout(() => {
      changed.dispose()
      reject(new Error(`Diagnostic update timed out for ${uri.toString()}`))
    }, 15_000)
    check()
  })
}

export async function workbenchText(
  action: {
    button?: string
    dismiss?: boolean
    contains?: string
    error?: boolean
    status?: string
    pages?: string
    page?: string
    pagesMessage?: string
    capture?: boolean
  },
): Promise<string> {
  const response = await fetch(process.env.TOLA_TEST_DRIVER!, {
    method: 'POST',
    body: JSON.stringify(action),
    signal: AbortSignal.timeout(20_000),
  })
  assert.equal(response.ok, true, await response.clone().text())
  const reply: unknown = await response.json()
  assert.ok(reply && typeof reply === 'object' && 'text' in reply && typeof reply.text === 'string')
  if ('capture' in reply && typeof reply.capture === 'string') {
    console.log(`Workbench screenshot: ${reply.capture}`)
  }
  return reply.text
}

export type OpenedPreview = { url: URL; status: number; html: string }

export async function withPreview(
  assertion: (opened: OpenedPreview[], firstOpen: Promise<OpenedPreview>) => Promise<void>,
): Promise<void> {
  const original = vscode.env.openExternal
  const opened: OpenedPreview[] = []
  let observe!: (page: OpenedPreview) => void
  const firstOpen = new Promise<OpenedPreview>((resolve) => {
    observe = resolve
  })
  vscode.env.openExternal = async (uri) => {
    const url = new URL(uri.toString())
    const response = await fetch(url, { signal: AbortSignal.timeout(15_000) })
    const page = { url, status: response.status, html: await response.text() }
    opened.push(page)
    observe(page)
    return response.ok
  }
  try {
    await assertion(opened, firstOpen)
  } finally {
    vscode.env.openExternal = original
  }
}

/** Open one source's page through `command`, proving the editor reached the site's published page. */
export async function openPublishedPage(
  command: 'tola.openPage' | 'tola.openPreview',
  source: vscode.Uri,
  opened: OpenedPreview[],
): Promise<OpenedPreview> {
  await vscode.window.showTextDocument(source)
  await vscode.commands.executeCommand(command, source)
  const page = opened.at(-1)
  assert.ok(page, `${command} opened no page`)
  assert.equal(page.status, 200)
  assert.match(page.html, /Published editor page\./)
  return page
}
