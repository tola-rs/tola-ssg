import * as vscode from 'vscode'
import { Preview } from './preview.ts'
import { Sites } from './sites.ts'

let sites: Sites | undefined
let preview: Preview | undefined

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel('Tola')
  const connections: Sites = new Sites(
    output,
    (selection) => previews.stop(selection),
    () => previews.refreshStatus(),
  )
  const previews: Preview = new Preview(
    output,
    (uri, siteKey) => connections.previewSite(uri, siteKey),
    (error) => connections.report(error),
  )
  preview = previews
  sites = connections
  context.subscriptions.push(
    output,
    vscode.commands.registerCommand('tola.showLog', () => output.show()),
    connections.register(),
    previews.register(),
  )
  await connections.refresh()
  previews.refreshStatus()
}

export async function deactivate(): Promise<void> {
  const connections = sites
  const previews = preview
  sites = undefined
  preview = undefined
  await Promise.all([previews?.close(), connections?.close()])
}
