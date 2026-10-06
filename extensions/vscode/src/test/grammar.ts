import * as assert from 'node:assert/strict'
import * as fs from 'node:fs/promises'
import * as path from 'node:path'
import * as vscode from 'vscode'
import { check } from './site.ts'

// The suite runs bundled as CommonJS inside the extension host: `require` exists at runtime and
// reaches the installation's own tokenizer, but this module's types do not declare it.
declare const require: (specifier: string) => unknown
/** The fence-language tags the extension must embed: Typst's own raw tags for markup, code, and
 * math, plus `example`, the tag Typst's documentation fences its examples with. */
const TAGS = ['typst', 'typ', 'typc', 'typm', 'example']

/** One line of a markdown document as the TextMate pipeline tokenizes it. */
type TokenizedLine = { text: string; tokens: Token[] }

type Token = { startIndex: number; endIndex: number; scopes: string[] }

/** The oniguruma module of VS Code's own installation. */
type Oniguruma = {
  loadWASM(data: ArrayBuffer): Promise<unknown>
  createOnigScanner(patterns: string[]): unknown
  createOnigString(text: string): unknown
}

/** The TextMate engine of VS Code's own installation, so a fence tokenizes exactly as the editor
 * renders it. */
type TextMate = {
  INITIAL: StateStack
  parseRawGrammar(source: string, filePath: string): RawGrammar
  Registry: new (options: {
    onigLib: Promise<OnigLibrary>
    loadGrammar(scopeName: string): Promise<RawGrammar | null>
    getInjections?(scopeName: string): string[] | undefined
  }) => Registry
}

type RawGrammar = { scopeName: string; injectionSelector?: string }
type StateStack = unknown
type OnigLibrary = {
  createOnigScanner(patterns: string[]): unknown
  createOnigString(text: string): unknown
}
type Registry = { loadGrammar(scopeName: string): Promise<Grammar | null> }
type Grammar = {
  tokenizeLine(line: string, state: StateStack | null): { tokens: Token[]; ruleStack: StateStack }
}

/** The scope the extension's injection grammar names its fenced Typst blocks with: the marker every
 * embedded token carries. */
const EMBEDDED_SCOPE = 'meta.embedded.block.typst'

let markdownGrammar: Promise<Grammar> | undefined

/** The installation's markdown grammar, with this extension's injection wired the way
 * `package.json` declares it: `markdown.typst.codeblock` injects into `text.html.markdown`. */
async function loadMarkdown(extensionPath: string): Promise<Grammar> {
  const bundled = path.join(vscode.env.appRoot, 'node_modules.asar')
  const textmate = require(path.join(bundled, 'vscode-textmate')) as TextMate
  const oniguruma = require(path.join(bundled, 'vscode-oniguruma')) as Oniguruma
  const grammar = async (scopeName: string): Promise<RawGrammar | null> => {
    if (scopeName === 'text.html.markdown') {
      return textmate.parseRawGrammar(
        await fs.readFile(
          path.join(vscode.env.appRoot, 'extensions/markdown-basics/syntaxes/markdown.tmLanguage.json'),
          'utf8',
        ),
        'markdown.tmLanguage.json',
      )
    }
    if (scopeName === 'source.typst') {
      return textmate.parseRawGrammar(
        await fs.readFile(path.join(extensionPath, 'syntaxes/typst.tmLanguage.json'), 'utf8'),
        'typst.tmLanguage.json',
      )
    }
    if (scopeName === 'markdown.typst.codeblock') {
      return textmate.parseRawGrammar(
        await fs.readFile(path.join(extensionPath, 'syntaxes/typst-markdown-injection.json'), 'utf8'),
        'typst-markdown-injection.json',
      )
    }
    return null
  }
  await oniguruma.loadWASM(
    (await fs.readFile(path.join(bundled, 'vscode-oniguruma/release/onig.wasm'))).buffer as ArrayBuffer,
  )
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => oniguruma.createOnigScanner(patterns),
      createOnigString: (text) => oniguruma.createOnigString(text),
    }),
    loadGrammar: grammar,
    getInjections: (scopeName) =>
      scopeName === 'text.html.markdown' ? ['markdown.typst.codeblock'] : undefined,
  })
  const loaded = await registry.loadGrammar('text.html.markdown')
  assert.ok(loaded, 'The installation answered no markdown grammar')
  return loaded
}

/** Every line of `markdown`, tokenized from the document's start. */
async function tokenizeLines(extensionPath: string, markdown: string): Promise<TokenizedLine[]> {
  markdownGrammar ??= loadMarkdown(extensionPath)
  const grammar = await markdownGrammar
  let state: StateStack = null
  return markdown.split('\n').map((text) => {
    const result = grammar.tokenizeLine(text, state)
    state = result.ruleStack
    return { text, tokens: result.tokens }
  })
}

/** The tokens of one fenced code block's body line, written with `tag` as the fence language. */
async function bodyTokens(extensionPath: string, tag: string, body: string): Promise<Token[]> {
  const lines = await tokenizeLines(extensionPath, ['```' + tag, body, '```', ''].join('\n'))
  const line = lines[1]
  assert.ok(line, `The ${tag} fence produced no body line`)
  assert.equal(line.text, body)
  return line.tokens
}

export async function runGrammarChecks(extensionPath: string): Promise<void> {
  await check('Every Typst raw tag highlights in a markdown fence', async () => {
    for (const tag of TAGS) {
      const tokens = await bodyTokens(extensionPath, tag, '#let value = 1 // note')
      for (const token of tokens) {
        assert.ok(
          token.scopes.includes(EMBEDDED_SCOPE),
          `The ${tag} fence body left the typst block: ${token.scopes.join(' ')}`,
        )
      }
      assert.ok(
        tokens.some((token) =>
          token.scopes.some((scope) => scope !== EMBEDDED_SCOPE && scope.endsWith('.typst'))
        ),
        `The ${tag} fence body reached no Typst rule`,
      )
    }
  })

  await check('Foreign fence languages stay outside the Typst grammar', async () => {
    for (const tag of ['python', 'typescript']) {
      const tokens = await bodyTokens(extensionPath, tag, '#let value = 1 // note')
      for (const token of tokens) {
        assert.ok(
          token.scopes.every((scope) => !scope.includes('typst')),
          `The ${tag} fence body carried a Typst scope: ${token.scopes.join(' ')}`,
        )
      }
    }
  })
}
