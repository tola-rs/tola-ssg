import { expect } from '@playwright/test'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { proxyEnvironment, startHeldProxy, startRefusingProxy } from '../../support/proxy.ts'
import { LanguageConnection, markedSource } from '../../support/lsp.ts'
import { expectExited, runCommand, test } from '../../support/process.ts'
import { contentDocument, writeMinimalSite, writeRenderingSite } from '../../support/site.ts'

type Position = { line: number; character: number }
type Range = { start: Position; end: Position }
type TextEdit = { range: Range; newText: string }
type Tokens = { data: number[]; resultId: string }
type TokenDelta = { edits: { start: number; deleteCount: number; data?: number[] }[] }
type SemanticLegend = { tokenTypes: string[]; tokenModifiers: string[] }
type DecodedToken = { text: string; type: string; modifiers: string[] }
type Handshake = { capabilities: { semanticTokensProvider: { legend: SemanticLegend } } }
type RouteReply = { routes: { output: string; route: string }[] }
type HierarchyItem = { name: string; uri: string; data?: unknown }
type IncomingCall = { from: HierarchyItem; fromRanges: Range[] }
type DocumentReport = {
  kind: 'full' | 'unchanged'
  resultId?: string
  items?: { code?: string; message: string }[]
}

async function withConnection(
  binary: string,
  root: string,
  assertion: (connection: LanguageConnection, handshake: Handshake) => Promise<void>,
  capabilities?: Record<string, unknown>,
): Promise<void> {
  const connection = new LanguageConnection(binary, root)
  try {
    const handshake = await connection.initialize<Handshake>(capabilities)
    await assertion(connection, handshake)
    await connection.close()
  } finally {
    await connection.terminate()
  }
}

function offsetAt(text: string, position: Position): number {
  const lines = text.split('\n')
  return lines.slice(0, position.line).reduce((offset, line) => offset + line.length + 1, 0) +
    position.character
}

function applyEdits(text: string, edits: TextEdit[]): string {
  for (
    const edit of [...edits].sort((left, right) =>
      offsetAt(text, right.range.start) - offsetAt(text, left.range.start)
    )
  ) {
    text = text.slice(0, offsetAt(text, edit.range.start)) + edit.newText +
      text.slice(offsetAt(text, edit.range.end))
  }
  return text
}

/** The tokens of one source, decoded from the protocol's relative encoding. */
function decodeTokens(text: string, data: number[], legend: SemanticLegend): DecodedToken[] {
  const starts = [0]
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === '\n') starts.push(index + 1)
  }
  // A token is five integers, and both the line table and the legend index every one of them.
  const decoded: DecodedToken[] = []
  let line = 0
  let character = 0
  for (let index = 0; index + 5 <= data.length; index += 5) {
    const deltaLine = data[index]!
    const deltaCharacter = data[index + 1]!
    const length = data[index + 2]!
    const type = data[index + 3]!
    const modifiers = data[index + 4]!
    line += deltaLine
    character = deltaLine === 0 ? character + deltaCharacter : deltaCharacter
    const lineStart = starts[line]
    const tokenType = legend.tokenTypes[type]
    if (lineStart === undefined || tokenType === undefined) {
      throw new Error('A semantic token names a line or type outside its legend')
    }
    decoded.push({
      text: text.slice(lineStart + character, lineStart + character + length),
      type: tokenType,
      modifiers: legend.tokenModifiers.filter((_, bit) => (modifiers & (1 << bit)) !== 0),
    })
  }
  return decoded
}

for (const mode of ['--pure', '--offline']) {
  test(`${mode} keeps language requests offline`, async ({ binary, directory: root }) => {
    await writeRenderingSite(root)
    const proxy = await startRefusingProxy()
    const connection = new LanguageConnection(binary, root, {
      ...proxyEnvironment(proxy.url),
      TYPST_PACKAGE_PATH: join(root, 'packages'),
      TYPST_PACKAGE_CACHE_PATH: join(root, 'cache'),
    }, mode)
    try {
      await connection.initialize()
      const uri = contentDocument(root, 'packages.typ').uri
      connection.open(uri, '#import "@preview/tola-scope-missing:1.0.0": *\n')
      await connection.waitForDiagnostics(
        uri,
        1,
        (diagnostics) => diagnostics.some((diagnostic) => diagnostic.code === 'typst.compile'),
      )
      const { text, position } = markedSource('#import "@preview/|"\n')
      connection.change(uri, 2, text)
      await connection.sendRequest('textDocument/completion', { textDocument: { uri }, position })
      await connection.close()
      expect(proxy.requests).toEqual([])
    } finally {
      await connection.terminate()
      await proxy.close()
    }
  })
}

test('source requests answer while the site check hangs', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const source = '#import "@preview/tola-e2e-missing:0.0.1": *\n#let leaf = [body]\n'
  const { path, uri } = contentDocument(root)
  await writeFile(path, source)

  const proxy = await startHeldProxy()
  const connection = new LanguageConnection(binary, root, {
    ...proxyEnvironment(proxy.url),
    TYPST_PACKAGE_PATH: join(root, 'packages'),
    TYPST_PACKAGE_CACHE_PATH: join(root, 'cache'),
  }, null)
  try {
    await connection.initialize()
    connection.open(uri, source)
    await expect.poll(() => proxy.tunnels.length, { timeout: 15_000 }).toBeGreaterThan(0)
    connection.send({ id: 'check', method: 'tola/route', params: { uri } })
    const tokens = await connection.sendRequest<Tokens>('textDocument/semanticTokens/full', {
      textDocument: { uri },
    })
    expect(tokens.data.length).toBeGreaterThan(0)
    expect(connection.received((message) => message.id === 'check')).toBe(false)
    expect(connection.received((message) => message.method === 'textDocument/publishDiagnostics')).toBe(false)

    connection.send({ method: '$/cancelRequest', params: { id: 'check' } })
    expect((await connection.waitFor((message) => message.id === 'check')).error)
      .toMatchObject({ code: -32800, message: 'request cancelled' })

    for (const tunnel of proxy.tunnels) tunnel.destroy()
    await connection.waitForDiagnostics(
      uri,
      1,
      (published) => published.some((diagnostic) => diagnostic.code === 'typst.compile'),
    )
    await connection.close()
  } finally {
    await connection.terminate()
    await proxy.close()
  }
})

test('Unicode frames retain request identity', async ({ binary, directory: root }) => {
  await writeMinimalSite(root)
  await withConnection(binary, root, async (connection) => {
    const unicode = contentDocument(root, 'unicode.typ').uri
    const ascii = contentDocument(root, 'ascii.typ').uri
    connection.open(unicode, '= 世界 𝄞\n')
    connection.open(ascii, '= Separate document\n')
    const replies = await Promise.all(
      [unicode, ascii].map((uri) =>
        connection.sendRequest<{ name: string }[]>('textDocument/documentSymbol', { textDocument: { uri } })
      ),
    )
    expect(replies.map((symbols) => symbols.map((symbol) => symbol.name)))
      .toEqual([['世界 𝄞'], ['Separate document']])
  })
})

test('unsaved diagnostics leave disk untouched', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  await writeFile(path, 'Saved document body.\n')
  await mkdir(join(root, 'public'))
  await writeFile(join(root, 'public/index.html'), '<p>Published revision</p>')

  await withConnection(binary, root, async (connection) => {
    connection.open(uri, '#undefined_from_unsaved_buffer\n')
    const errors = await connection.waitForDiagnostics(
      uri,
      1,
      (diagnostics) =>
        diagnostics.some((diagnostic) => diagnostic.message.includes('undefined_from_unsaved_buffer')),
    )
    expect(errors.map((diagnostic) => diagnostic.code)).toContain('typst.compile')
    connection.change(uri, 2, 'Unsaved body.\n')
    expect(await connection.waitForDiagnostics(uri, 2, (diagnostics) => diagnostics.length === 0)).toEqual([])
    expect(await readFile(path, 'utf8')).toBe('Saved document body.\n')
    expect(await readFile(join(root, 'public/index.html'), 'utf8')).toBe('<p>Published revision</p>')
  })
})

test('untitled changes preserve site routes', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  await writeFile(path, 'Published document.\n')
  await withConnection(binary, root, async (connection, handshake) => {
    const routes = await connection.sendRequest<RouteReply>('tola/route', { uri })
    expect(routes.routes).toEqual([{ output: 'document/index.html', route: '/document/' }])
    const untitled = 'untitled:Unsaved-1'
    connection.open(untitled, '= Draft\n')
    connection.change(untitled, 2, '= Unsaved 𝄞\n#unknown_binding\n')
    const tokens = await connection.sendRequest<Tokens>('textDocument/semanticTokens/full', {
      textDocument: { uri: untitled },
    })
    const heading = handshake.capabilities.semanticTokensProvider.legend.tokenTypes.indexOf('heading')
    expect(tokens.data.slice(0, 4)).toEqual([0, 0, '= Unsaved 𝄞'.length, heading])
    expect(await connection.sendRequest<RouteReply>('tola/route', { uri })).toEqual(routes)
    connection.send({ method: 'textDocument/didClose', params: { textDocument: { uri: untitled } } })
    expect(await connection.sendRequest<RouteReply>('tola/route', { uri })).toEqual(routes)
  })
})

test('plain completions omit placeholders', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  await writeFile(path, 'Saved body.\n')
  await withConnection(binary, root, async (connection) => {
    const source = markedSource('𝄞 #rep|\n')
    connection.open(uri, source.text)
    const completions = await connection.sendRequest<{
      label: string
      insertTextFormat?: number
      textEdit?: TextEdit
      insertText?: string
    }[]>('textDocument/completion', { textDocument: { uri }, position: source.position })
    const repr = completions.find((completion) => completion.label === 'repr')
    expect(repr).toBeDefined()
    expect(repr!.insertTextFormat ?? 1).toBe(1)
    expect(repr!.textEdit).toBeDefined()
    expect(applyEdits(source.text, [repr!.textEdit!])).toBe('𝄞 #repr()\n')
  }, { textDocument: { completion: { completionItem: { snippetSupport: false } } } })
})

test('plaintext hover honors negotiation', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  await writeFile(path, 'Saved body.\n')
  await withConnection(binary, root, async (connection) => {
    const position = connection.openMarked(uri, '𝄞 #lo|rem(1)\n')
    const hover = await connection.sendRequest<{ contents: { kind: string; value: string } }>(
      'textDocument/hover',
      { textDocument: { uri }, position },
    )
    expect(hover.contents.kind).toBe('plaintext')
  }, { textDocument: { hover: { contentFormat: ['plaintext'] } } })
})

test('flat outlines carry document locations', async ({ binary, directory: root }) => {
  await writeMinimalSite(root)
  const { uri } = contentDocument(root)
  await withConnection(binary, root, async (connection) => {
    connection.open(uri, '= Parent\n== Child 𝄞\n')
    const symbols = await connection.sendRequest<{
      name: string
      children?: unknown
      location: { uri: string; range: Range }
    }[]>('textDocument/documentSymbol', { textDocument: { uri } })
    expect(symbols.map((symbol) => symbol.name)).toEqual(['Parent', 'Child 𝄞'])
    expect(symbols.every((symbol) => symbol.location?.uri === uri && symbol.children === undefined)).toBe(
      true,
    )
  }, { textDocument: { documentSymbol: { hierarchicalDocumentSymbolSupport: false } } })
})

test('token deltas use the requested base', async ({ binary, directory: root }) => {
  await writeMinimalSite(root)
  const { uri } = contentDocument(root)
  await withConnection(binary, root, async (connection) => {
    connection.open(uri, '= Heading\n')
    const earlier = await connection.sendRequest<Tokens>('textDocument/semanticTokens/full', {
      textDocument: { uri },
    })
    connection.change(uri, 2, '= Heading\n#let count = 1\n#count\n')
    const current = await connection.sendRequest<Tokens>('textDocument/semanticTokens/full', {
      textDocument: { uri },
    })
    expect(current.data).not.toEqual(earlier.data)
    const reply = await connection.sendRequest<Tokens | TokenDelta>(
      'textDocument/semanticTokens/full/delta',
      {
        textDocument: { uri },
        previousResultId: earlier.resultId,
      },
    )
    const applied = 'data' in reply ? reply.data : earlier.data.slice()
    if ('edits' in reply) {
      for (const edit of [...reply.edits].reverse()) {
        applied.splice(edit.start, edit.deleteCount, ...(edit.data ?? []))
      }
    }
    expect(applied).toEqual(current.data)
  })
})

test('math groups keep their leaf tokens', async ({ binary, directory: root }) => {
  await writeMinimalSite(root)
  const { uri } = contentDocument(root)
  const source = '$(alpha + beta)$\n$[gamma / delta]$\n$sin x$\n'
  await withConnection(binary, root, async (connection, handshake) => {
    connection.open(uri, source)
    const tokens = await connection.sendRequest<Tokens>('textDocument/semanticTokens/full', {
      textDocument: { uri },
    })
    const decoded = decodeTokens(source, tokens.data, handshake.capabilities.semanticTokensProvider.legend)
    // A group keeps its leaves: `(alpha + beta)` never collapses into one text run.
    expect(decoded.some((token) => token.text.includes('alpha + beta'))).toBe(false)
    expect(decoded.every((token) => token.modifiers.includes('math'))).toBe(true)
    expect(decoded.filter((token) => token.text === 'alpha'))
      .toEqual([{ text: 'alpha', type: 'pol', modifiers: ['math'] }])
    expect(decoded.filter((token) => token.text === 'gamma'))
      .toEqual([{ text: 'gamma', type: 'pol', modifiers: ['math'] }])
    // The fraction bar is an operator of the group, and the library's math operator is a function.
    expect(decoded.filter((token) => token.text === '/'))
      .toEqual([{ text: '/', type: 'operator', modifiers: ['math'] }])
    expect(decoded.filter((token) => token.text === 'sin'))
      .toEqual([{ text: 'sin', type: 'function', modifiers: ['math'] }])
    expect(decoded.filter((token) => token.type === 'delim').map((token) => token.text))
      .toEqual(['$', '(', ')', '$', '$', '[', ']', '$', '$', '$'])
  })
})

test('package definitions open virtual sources', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  await writeFile(path, '')
  await withConnection(binary, root, async (connection) => {
    const position = connection.openMarked(
      uri,
      '#import "@tola/document:0.0.0": current-document as identity\n𝄞 #let value = ident|ity\n',
    )
    const target = await connection.sendRequest<{ uri: string; range: Range }>('textDocument/definition', {
      textDocument: { uri },
      position,
    })
    expect(target.uri).toBe('tola-package:/tola/document/0.0.0/lib.typ')
    const source = await connection.sendRequest<{ text: string }>('tola/source', { uri: target.uri })
    expect(
      source.text.slice(offsetAt(source.text, target.range.start), offsetAt(source.text, target.range.end)),
    )
      .toBe('current-document')
  })
})

test('local import rename preserves exports', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  await writeFile(join(root, 'greeting.typ'), '#let greeting(name) = [Hello, #name.]\n')
  await writeFile(
    join(root, 'content/neighbor.typ'),
    '#import "../greeting.typ": greeting\n#greeting("neighbor")\n',
  )
  const source = markedSource('#import "../greeting.typ": greeting\n𝄞 #greet|ing("reader")\n')
  const { path, uri } = contentDocument(root)
  await writeFile(path, source.text)
  await withConnection(binary, root, async (connection) => {
    connection.open(uri, source.text)
    const edit = await connection.sendRequest<
      { changes: Record<string, TextEdit[]>; documentChanges?: unknown }
    >(
      'textDocument/rename',
      { textDocument: { uri }, position: source.position, newName: 'welcome' },
    )
    expect(edit.documentChanges).toBeUndefined()
    for (const [changedUri, edits] of Object.entries(edit.changes)) {
      const filename = fileURLToPath(changedUri)
      await writeFile(filename, applyEdits(await readFile(filename, 'utf8'), edits))
    }
    const build = await runCommand(binary, ['build', '--pure'], root, 30_000)
    expectExited(build)
    expect(await readFile(join(root, 'public/document/index.html'), 'utf8')).toContain('Hello, reader.')
    expect(await readFile(join(root, 'public/neighbor/index.html'), 'utf8')).toContain('Hello, neighbor.')
  })
})

test('capturing rename reports a conflict', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  await writeFile(path, 'Saved body.\n')
  const collision = markedSource('#let original = 1\n#let occupied = 2\n𝄞 #orig|inal\n')
  const capture = markedSource('#let original = 1\n#{\n  let occupied = 2\n  orig|inal\n}\n')
  await withConnection(binary, root, async (connection) => {
    connection.open(uri, collision.text)
    connection.send({
      id: 'collision',
      method: 'textDocument/rename',
      params: {
        textDocument: { uri },
        position: collision.position,
        newName: 'occupied',
      },
    })
    expect((await connection.waitFor((message) => message.id === 'collision')).error)
      .toMatchObject({ code: -32602, data: { kind: 'renameConflict' } })

    // The declaration inside the block would capture this use of the outer name.
    connection.change(uri, 2, capture.text)
    connection.send({
      id: 'capture',
      method: 'textDocument/rename',
      params: {
        textDocument: { uri },
        position: capture.position,
        newName: 'occupied',
      },
    })
    const reply = await connection.waitFor((message) => message.id === 'capture')
    expect(reply.error).toMatchObject({ code: -32602, data: { kind: 'renameConflict' } })
    expect(reply.result).toBeUndefined()
  })
})

test('configuration diagnostics follow edits', async ({ binary, directory: root }) => {
  await writeMinimalSite(root)
  const uri = pathToFileURL(join(root, 'tola.toml')).href
  await withConnection(binary, root, async (connection) => {
    connection.open(uri, '[site\n')
    await connection.waitForDiagnostics(
      uri,
      1,
      (published) => published.some((diagnostic) => diagnostic.code === 'config.toml'),
    )
    connection.change(uri, 2, '')
    expect(await connection.waitForDiagnostics(uri, 2, (published) => published.length === 0)).toEqual([])
  })
})

test('interrupt stops the language server', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Node pipes do not deliver Windows console interrupts.')
  await writeMinimalSite(root)
  const connection = new LanguageConnection(binary, root)
  try {
    await connection.initialize()
    // stdin stays open: only the interrupt can end this session.
    expect(await connection.interrupt()).toEqual({ code: 130, signal: null })
  } finally {
    await connection.terminate()
  }
})

test('call hierarchy names the page that includes a partial', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const partial = contentDocument(root, 'partial.typ')
  const page = contentDocument(root, 'page.typ')
  await writeFile(partial.path, '= Shared note\n')
  await writeFile(page.path, '#include "partial.typ"\n\nPage body.\n')

  await withConnection(binary, root, async (connection) => {
    connection.open(partial.uri, await readFile(partial.path, 'utf8'))
    connection.open(page.uri, await readFile(page.path, 'utf8'))

    const items = await connection.sendRequest<HierarchyItem[]>('textDocument/prepareCallHierarchy', {
      textDocument: { uri: partial.uri },
      position: { line: 0, character: 0 },
    })
    expect(items.map((item) => item.name)).toEqual(['content/partial.typ'])
    expect(items[0]?.uri).toBe(partial.uri)

    const incoming = await connection.sendRequest<IncomingCall[]>('callHierarchy/incomingCalls', {
      item: items[0],
    })
    expect(incoming.map((call) => call.from.name)).toEqual(['content/page.typ'])
    expect(incoming[0]?.from.uri).toBe(page.uri)
    expect(incoming[0]?.fromRanges).toHaveLength(1)
  })
})

test('a pull-only client reads diagnostics without pushed reports', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const document = contentDocument(root, 'broken.typ')
  const text = '#let broken = nosuchname\n'
  await writeFile(document.path, text)
  const uri = document.uri

  await withConnection(binary, root, async (connection) => {
    connection.open(uri, text)
    // The refresh request follows the report the check produced, so a client that hears it is
    // reading a report rather than racing the check.
    await connection.waitForServerRequest('workspace/diagnostic/refresh')
    expect(connection.received((message) => message.method === 'textDocument/publishDiagnostics'))
      .toBe(false)

    const report = await connection.sendRequest<DocumentReport>('textDocument/diagnostic', {
      textDocument: { uri },
    })
    expect(report.kind).toBe('full')
    expect(report.items?.map((diagnostic) => diagnostic.code)).toContain('typst.compile')

    const repeated = await connection.sendRequest<DocumentReport>('textDocument/diagnostic', {
      textDocument: { uri },
      previousResultId: report.resultId,
    })
    expect(repeated.kind).toBe('unchanged')
    expect(repeated.resultId).toBe(report.resultId)
  }, {
    textDocument: { diagnostic: {}, publishDiagnostics: { versionSupport: true } },
    workspace: { diagnostic: { refreshSupport: true } },
  })
})

test('a pull for a document the check has not reached is refused', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const document = contentDocument(root, 'broken.typ')
  const uri = document.uri
  await writeFile(document.path, '#let broken = nosuchname\n')

  const proxy = await startHeldProxy()
  const connection = new LanguageConnection(binary, root, {
    ...proxyEnvironment(proxy.url),
    TYPST_PACKAGE_PATH: join(root, 'packages'),
    TYPST_PACKAGE_CACHE_PATH: join(root, 'cache'),
  }, null)
  try {
    await connection.initialize({
      textDocument: { diagnostic: {} },
      workspace: { diagnostic: { refreshSupport: true } },
    })
    connection.open(uri, '#let broken = nosuchname\n')
    await connection.waitForServerRequest('workspace/diagnostic/refresh')
    const report = await connection.sendRequest<DocumentReport>('textDocument/diagnostic', {
      textDocument: { uri },
    })
    expect(report.kind).toBe('full')

    connection.change(uri, 2, '#import "@preview/tola-e2e-missing:0.0.1": *\n#let broken = nosuchname\n')
    await expect.poll(() => proxy.tunnels.length, { timeout: 15_000 }).toBeGreaterThan(0)

    connection.send({
      id: 'refused',
      method: 'textDocument/diagnostic',
      params: { textDocument: { uri }, previousResultId: report.resultId },
    })
    const refused = await connection.waitFor((message) => message.id === 'refused')
    expect(refused.error).toMatchObject({ code: -32802, data: { retriggerRequest: true } })

    for (const tunnel of proxy.tunnels) tunnel.destroy()
    await connection.close()
  } finally {
    await connection.terminate()
    await proxy.close()
  }
})

test('the client registers a watcher for the site sources', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  await withConnection(binary, root, async (connection) => {
    const registration = await connection.waitForServerRequest('client/registerCapability')
    expect(registration.params?.registrations?.[0]).toMatchObject({
      method: 'workspace/didChangeWatchedFiles',
      registerOptions: { watchers: [{ globPattern: '**/*' }] },
    })
  }, { workspace: { didChangeWatchedFiles: { dynamicRegistration: true } } })
})

test('an unopened source the client reports is checked', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const document = contentDocument(root, 'later.typ')
  const uri = document.uri

  await withConnection(binary, root, async (connection) => {
    await writeFile(document.path, '#let later = nosuchname\n')
    connection.send({
      method: 'workspace/didChangeWatchedFiles',
      params: { changes: [{ uri, type: 1 }] },
    })
    await connection.waitForDiagnostics(
      uri,
      null,
      (published) => published.some((diagnostic) => diagnostic.code === 'typst.compile'),
    )
  }, { workspace: { didChangeWatchedFiles: { dynamicRegistration: true } } })
})

test('source status follows saved revisions', async ({ binary, directory: root }) => {
  await writeRenderingSite(root)
  const { path, uri } = contentDocument(root)
  const warning = '#set text(font: "Tola E2E absent font")\nWarning-bearing source.\n'
  await writeFile(path, warning)
  const connection = new LanguageConnection(binary, root)
  const status = async (state: string, revision = -1) => {
    const message = await connection.waitFor((message) =>
      message.method === 'tola/sourceCheckStatus' && message.params?.state === state &&
      (message.params.revision ?? -1) >= revision
    )
    return message.params!.revision!
  }
  try {
    await connection.initialize(undefined, { sourceCheckStatus: true, checkMode: 'onSave' })
    await status('checking')
    await status('checked')
    connection.open(uri, warning)
    const opened = await status('checking')
    await status('checked', opened)
    await connection.waitForDiagnostics(
      uri,
      1,
      (diagnostics) => diagnostics.some((diagnostic) => diagnostic.message.includes('unknown font family')),
    )
    connection.change(uri, 2, '#panic("Unsaved source failure")\n')
    const dirty = await status('notChecked', opened + 1)
    await connection.sendRequest('textDocument/documentSymbol', { textDocument: { uri } })
    expect(
      connection.received((message) =>
        message.method === 'tola/sourceCheckStatus' && message.params?.state === 'checked' &&
        (message.params.revision ?? -1) >= dirty
      ),
    ).toBe(false)
    connection.send({ method: 'textDocument/didSave', params: { textDocument: { uri } } })
    const checking = await status('checking', dirty)
    await status('failed', checking)
    await connection.waitForDiagnostics(
      uri,
      2,
      (diagnostics) =>
        diagnostics.some((diagnostic) => diagnostic.message.includes('Unsaved source failure')),
    )
    connection.change(uri, 3, warning)
    const recovery = await status('notChecked', checking + 1)
    connection.send({ method: 'textDocument/didSave', params: { textDocument: { uri } } })
    await status('checking', recovery)
    await status('checked', recovery)
    await connection.close()
  } finally {
    await connection.terminate()
  }
})

test('a workspace without a configuration answers about its documents', async ({ binary, directory: root }) => {
  const document = contentDocument(root)
  await mkdir(join(root, 'content'), { recursive: true })
  const text = '#let nothing = undefined-thing\n'
  await writeFile(document.path, text)
  const uri = document.uri

  await withConnection(binary, root, async (connection) => {
    connection.open(uri, text)
    const diagnostics = await connection.waitForDiagnostics(
      uri,
      1,
      (published) => published.some((diagnostic) => diagnostic.code === 'typst.compile'),
    )
    expect(diagnostics.map((diagnostic) => diagnostic.message).join('\n'))
      .toContain('undefined-thing')
  })
})

test('a workspace without a configuration completes names', async ({ binary, directory: root }) => {
  const document = contentDocument(root)
  await mkdir(join(root, 'content'), { recursive: true })
  await writeFile(document.path, 'Document body.\n')
  const uri = document.uri

  await withConnection(binary, root, async (connection) => {
    const position = connection.openMarked(uri, '#rep|\n')
    const completions = await connection.sendRequest<{ label: string }[]>(
      'textDocument/completion',
      { textDocument: { uri }, position },
    )
    expect(completions.map((completion) => completion.label)).toContain('repr')
  }, { textDocument: { completion: { completionItem: { snippetSupport: false } } } })
})
