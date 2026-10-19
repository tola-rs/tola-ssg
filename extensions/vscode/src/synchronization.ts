import * as vscode from 'vscode'

/** A service acknowledges only the document versions its own transport has sent. */
export class SourceSynchronization implements vscode.Disposable {
  private readonly versions = new Map<vscode.TextDocument, number>()
  private readonly pending = new Map<string, Promise<void>>()
  private readonly changed = new vscode.EventEmitter<
    {
      document: vscode.TextDocument
      version: number | undefined
    } | undefined
  >()
  private disposed = false

  open(document: vscode.TextDocument, send: () => Promise<void>): Promise<void> {
    return this.enqueue(document, async () => {
      const version = document.version
      await send()
      return version
    })
  }

  change(document: vscode.TextDocument, version: number, send: () => Promise<void>): Promise<void> {
    return this.enqueue(document, async () => {
      await send()
      return version
    })
  }

  close(document: vscode.TextDocument, send: () => Promise<void>): Promise<void> {
    return this.enqueue(document, async () => {
      await send()
      return undefined
    })
  }

  save(document: vscode.TextDocument, send: () => Promise<void>): Promise<void> {
    return this.enqueue(document, async () => {
      await send()
      return this.versions.get(document)
    })
  }

  private enqueue(
    document: vscode.TextDocument,
    send: () => Promise<number | undefined>,
  ): Promise<void> {
    const uri = document.uri.toString()
    const previous = this.pending.get(uri) ?? Promise.resolve()
    const sent = previous.then(async () => {
      if (this.disposed) return
      try {
        const version = await send()
        if (this.disposed) return
        if (version === undefined) this.versions.delete(document)
        else this.versions.set(document, version)
        this.changed.fire({ document, version })
      } catch (error) {
        this.versions.delete(document)
        this.changed.fire({ document, version: undefined })
        throw error
      }
    })
    const settled = sent.then(() => {}, () => {}).then(() => {
      if (this.pending.get(uri) === settled) this.pending.delete(uri)
    })
    this.pending.set(uri, settled)
    return sent
  }

  wait(document: vscode.TextDocument, version: number, token: vscode.CancellationToken): Promise<boolean> {
    if (this.disposed || document.isClosed || token.isCancellationRequested) return Promise.resolve(false)
    if ((this.versions.get(document) ?? -1) >= version) return Promise.resolve(true)
    return new Promise<boolean>((resolve) => {
      const finish = (sent: boolean) => {
        changed.dispose()
        cancelled.dispose()
        resolve(sent)
      }
      const changed = this.changed.event((sent) => {
        if (!sent) return finish(false)
        if (sent.document !== document) return
        if (sent.version === undefined) finish(false)
        else if (sent.version >= version) finish(true)
      })
      const cancelled = token.onCancellationRequested(() => finish(false))
    })
  }

  dispose(): void {
    this.disposed = true
    this.versions.clear()
    this.pending.clear()
    this.changed.fire(undefined)
    this.changed.dispose()
  }
}
