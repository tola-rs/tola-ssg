import { EventEmitter } from 'node:events'
import { pathToFileURL } from 'node:url'
import { RunningProcess } from './process.ts'

export type RpcMessage = {
  id?: string | number
  method?: string
  params?: {
    uri?: string
    version?: number
    diagnostics?: { code?: string; message: string }[]
    revision?: number
    state?: 'notChecked' | 'checking' | 'checked' | 'failed'
  }
  result?: unknown
  error?: unknown
}
/** One request the server sends the client; the harness answers it as unsupported. */
export type ServerRequest = {
  id: string | number
  method: string
  params?: {
    registrations?: { method?: string; registerOptions?: { watchers?: { globPattern?: string }[] } }[]
  }
}

/** `|` marks the cursor position within `text`. */
export type MarkedSource = {
  text: string
  position: { line: number; character: number }
}

export function markedSource(marked: string): MarkedSource {
  const offset = marked.indexOf('|')
  if (offset < 0) throw new Error('A marked source carries a `|` cursor')
  const text = marked.slice(0, offset) + marked.slice(offset + 1)
  // JavaScript strings are UTF-16, so a line's length is exactly its LSP column.
  const line = text.slice(0, offset).split('\n').length - 1
  const character = offset - text.lastIndexOf('\n', offset - 1) - 1
  return { text, position: { line, character } }
}

export class LanguageConnection {
  private readonly command: RunningProcess
  private readonly root: string
  private pendingBytes = Buffer.alloc(0)
  private readonly messages: RpcMessage[] = []
  private readonly requests: ServerRequest[] = []
  private readonly events = new EventEmitter()
  private lastRequestId = 0
  private stderr = ''
  private failure: Error | undefined

  /**
   * `mode` is the command's input-scope flag; `null` runs the default online scope a client
   * without `--pure` or `--offline` gets.
   */
  constructor(
    binary: string,
    root: string,
    environment: NodeJS.ProcessEnv = {},
    mode: string | null = '--pure',
  ) {
    this.command = new RunningProcess(binary, mode === null ? ['lsp'] : ['lsp', mode], root, environment)
    this.root = root
    const child = this.command.child
    const channels: EventEmitter[] = [child, child.stdin, child.stdout, child.stderr]
    const failed = (error: Error) => {
      this.failure ??= error
      this.events.emit('change')
    }
    const receive = (chunk: Buffer) => {
      if (this.failure) return
      try {
        this.receive(chunk)
      } catch (error) {
        failed(error instanceof Error ? error : new Error(String(error)))
      }
    }
    const captureStderr = (chunk: string) => {
      this.stderr += chunk
    }
    child.stderr.setEncoding('utf8')
    child.stderr.on('data', captureStderr)
    child.stdout.on('data', receive)
    for (const channel of channels) {
      channel.on('error', failed)
    }
    child.once('close', () => {
      child.stderr.removeListener('data', captureStderr)
      child.stdout.removeListener('data', receive)
      for (const channel of channels) {
        channel.removeListener('error', failed)
      }
      if (this.pendingBytes.length !== 0 && !this.failure) {
        failed(new Error('Language server closed with an incomplete message'))
      }
      this.events.emit('change')
    })
  }

  send(message: Record<string, unknown>): void {
    if (this.failure) throw this.failure
    if (this.command.child.stdin.destroyed) throw new Error('Language server input is closed')
    const bytes = Buffer.from(JSON.stringify({ jsonrpc: '2.0', ...message }))
    const header = Buffer.from(`Content-Length: ${bytes.length}\r\n\r\n`)
    this.command.child.stdin.write(Buffer.concat([header, bytes]))
  }

  async initialize<T = unknown>(
    capabilities: Record<string, unknown> = {
      textDocument: { publishDiagnostics: { versionSupport: true } },
    },
    initializationOptions: Record<string, unknown> = {},
  ): Promise<T> {
    const reply = await this.sendRequest<T>('initialize', {
      processId: process.pid,
      rootUri: pathToFileURL(this.root).href,
      capabilities,
      initializationOptions,
    })
    this.send({ method: 'initialized', params: {} })
    return reply
  }

  async sendRequest<T>(method: string, params: unknown): Promise<T> {
    const id = ++this.lastRequestId
    this.send({ id, method, params })
    const message = await this.waitFor((candidate) => candidate.id === id)
    if (message.error !== undefined) {
      throw new Error(`${method} failed: ${JSON.stringify(message.error)}`)
    }
    return message.result as T
  }

  open(uri: string, text: string): void {
    this.send({
      method: 'textDocument/didOpen',
      params: { textDocument: { uri, languageId: 'typst', version: 1, text } },
    })
  }

  openMarked(uri: string, marked: string): { line: number; character: number } {
    const source = markedSource(marked)
    this.open(uri, source.text)
    return source.position
  }

  change(uri: string, version: number, text: string): void {
    this.send({
      method: 'textDocument/didChange',
      params: { textDocument: { uri, version }, contentChanges: [{ text }] },
    })
  }

  /**
   * The report one document's push carries at `version`; a document no client has open is reported
   * without a version, which `null` matches.
   */
  async waitForDiagnostics(
    uri: string,
    version: number | null,
    accept: (diagnostics: { code?: string; message: string }[]) => boolean,
  ): Promise<{ code?: string; message: string }[]> {
    const message = await this.waitFor((candidate) =>
      candidate.method === 'textDocument/publishDiagnostics' &&
      candidate.params?.uri === uri &&
      (candidate.params.version ?? null) === version &&
      accept(candidate.params.diagnostics ?? [])
    )
    return message.params?.diagnostics ?? []
  }

  private receive(chunk: Buffer): void {
    this.pendingBytes = Buffer.concat([this.pendingBytes, chunk])
    for (;;) {
      const separator = this.pendingBytes.indexOf('\r\n\r\n')
      if (separator < 0) break
      const header = this.pendingBytes.subarray(0, separator).toString()
      const length = Number(header.match(/^content-length:[ \t]*(\d+)[ \t]*$/im)?.[1])
      if (!Number.isSafeInteger(length) || length < 0) {
        throw new Error(`Invalid language server header: ${header}`)
      }
      if (this.pendingBytes.length < separator + 4 + length) break
      const message: RpcMessage = JSON.parse(
        this.pendingBytes.subarray(separator + 4, separator + 4 + length).toString(),
      )
      if (message === null || typeof message !== 'object' || Array.isArray(message)) {
        throw new Error('Language server message must be a JSON object')
      }
      this.pendingBytes = this.pendingBytes.subarray(separator + 4 + length)
      if (message.method && message.id !== undefined) {
        this.requests.push(message as ServerRequest)
        this.send({
          id: message.id,
          error: { code: -32601, message: `Unsupported client method: ${message.method}` },
        })
      } else {
        this.messages.push(message)
      }
    }
    this.events.emit('change')
  }

  waitFor(predicate: (message: RpcMessage) => boolean): Promise<RpcMessage> {
    const { promise, resolve, reject } = Promise.withResolvers<RpcMessage>()
    const cleanup = () => {
      clearTimeout(timeout)
      this.events.removeListener('change', observe)
    }
    const failed = (error: Error) => {
      cleanup()
      reject(new Error(`Language server response failed:\n${this.stderr}`, { cause: error }))
    }
    const observe = () => {
      if (this.failure) {
        failed(this.failure)
        return
      }
      let index: number
      try {
        index = this.messages.findIndex(predicate)
      } catch (error) {
        failed(error instanceof Error ? error : new Error(String(error)))
        return
      }
      if (index >= 0) {
        cleanup()
        resolve(this.take(index))
      } else if (this.command.exit !== undefined) {
        failed(new Error('Language server exited before its response'))
      }
    }
    const timeout = setTimeout(() => failed(new Error('Language server response timed out')), 15_000)
    this.events.on('change', observe)
    observe()
    return promise
  }

  /** Removes the message at `index`, which the caller located by predicate. */
  private take(index: number): RpcMessage {
    const [message] = this.messages.splice(index, 1)
    if (message === undefined) throw new Error('The selected message was already taken')
    return message
  }

  /** Whether a matching message is already here, without waiting for one. */
  received(predicate: (message: RpcMessage) => boolean): boolean {
    return this.messages.some(predicate)
  }

  /** Waits for one request the server sends the client, which the harness answers as unsupported. */
  waitForServerRequest(method: string): Promise<ServerRequest> {
    const { promise, resolve, reject } = Promise.withResolvers<ServerRequest>()
    const cleanup = () => {
      clearTimeout(timeout)
      this.events.removeListener('change', observe)
    }
    const failed = (error: Error) => {
      cleanup()
      reject(new Error(`Language server request failed:\n${this.stderr}`, { cause: error }))
    }
    const observe = () => {
      if (this.failure) {
        failed(this.failure)
        return
      }
      const index = this.requests.findIndex((request) => request.method === method)
      if (index >= 0) {
        cleanup()
        const [request] = this.requests.splice(index, 1)
        resolve(request!)
      } else if (this.command.exit !== undefined) {
        failed(new Error('Language server exited before its request'))
      }
    }
    const timeout = setTimeout(() => failed(new Error(`Language server never sent ${method}`)), 15_000)
    this.events.on('change', observe)
    observe()
    return promise
  }

  async close(): Promise<void> {
    const child = this.command.child
    if (child.exitCode === null && child.signalCode === null) {
      await this.sendRequest('shutdown', null)
      this.send({ method: 'exit', params: null })
      child.stdin.end()
    }
    const exit = await this.command.waitForClose(5_000)
    if (exit.code !== 0) throw new Error(`language server exited with ${JSON.stringify(exit)}`)
    if (process.platform !== 'win32') {
      let missing: NodeJS.ErrnoException | undefined
      try {
        process.kill(-child.pid!, 0)
      } catch (error) {
        missing = error as NodeJS.ErrnoException
      }
      if (missing?.code !== 'ESRCH') throw new Error('language server left a process behind')
    }
  }

  interrupt(): Promise<{ code: number | null; signal: NodeJS.Signals | null }> {
    this.command.interrupt()
    return this.command.waitForClose(5_000)
  }

  async terminate(): Promise<void> {
    await this.command.terminate()
  }
}
