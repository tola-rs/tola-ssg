import * as vscode from 'vscode'

import type { Selection } from './selection.ts'
import type { SiteRoute } from './routes.ts'
import type { PreviewSite } from './preview.ts'

/** One site the view lists pages for. */
export type PagesSite = { readonly name: string; readonly site: PreviewSite }

export class PagesView implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly changed = new vscode.EventEmitter<void>()
  readonly onDidChangeTreeData = this.changed.event
  private view: vscode.TreeView<vscode.TreeItem> | undefined

  constructor(
    private readonly sites: () => readonly PagesSite[],
    private readonly output: vscode.OutputChannel,
  ) {}

  register(): vscode.Disposable {
    this.view = vscode.window.createTreeView('tolaPages', { treeDataProvider: this })
    return vscode.Disposable.from(
      this,
      this.view,
      vscode.commands.registerCommand('tola.refreshPages', () => this.refresh()),
    )
  }

  dispose(): void {
    this.changed.dispose()
  }

  /** Re-read every site's pages: a save, a config change, or the refresh command asks for it. */
  refresh(): void {
    this.updateCheckMessage()
    this.changed.fire()
  }

  updateCheckMessage(): void {
    if (!this.view) return
    const states = this.sites().map(({ site }) => site.check())
    this.view.message = states.includes('notChecked')
      ? 'Source changes not checked. Routes are from the last query; Save or Refresh Pages to update.'
      : states.includes('checking')
      ? 'Checking source changes. Routes are from the last query.'
      : states.includes('failed')
      ? 'Source check failed. Open Tola output for diagnostics.'
      : ''
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element
  }

  getChildren(element?: vscode.TreeItem): vscode.ProviderResult<vscode.TreeItem[]> {
    const [only, ...others] = this.sites()
    if (element instanceof PageItem) return []
    if (element instanceof SiteItem) return this.pages(element.site)
    if (only && !others.length) return this.pages(only.site)
    return this.sites().map(({ name, site }) => new SiteItem(name, site))
  }

  private async pages(site: PreviewSite): Promise<vscode.TreeItem[]> {
    try {
      const pages = await site.siteRoutes()
      return site.lifetime.isCancellationRequested
        ? []
        : pages.map((page) => new PageItem(page, site.selection))
    } catch (error) {
      if (site.lifetime.isCancellationRequested) return []
      this.output.appendLine(
        `Checked page request failed for ${site.selection.folder.name}: ${String(error)}`,
      )
      throw new Error(
        `Tola could not read checked pages for ${site.selection.folder.name}. Open Tola output or run Refresh Pages again`,
      )
    }
  }
}

class SiteItem extends vscode.TreeItem {
  constructor(name: string, readonly site: PreviewSite) {
    super(name, vscode.TreeItemCollapsibleState.Expanded)
    this.contextValue = 'tolaSite'
    this.iconPath = new vscode.ThemeIcon('globe')
    this.tooltip = `Checked pages for ${name}`
  }
}

class PageItem extends vscode.TreeItem {
  constructor(page: SiteRoute, selected: Selection) {
    super(page.route, vscode.TreeItemCollapsibleState.None)
    const source = page.source === undefined
      ? undefined
      : vscode.Uri.joinPath(vscode.Uri.file(selected.inputRoots[0]!), page.source)
    this.description = page.source ?? 'Site program'
    this.contextValue = 'tolaPage'
    this.iconPath = new vscode.ThemeIcon('file-media')
    this.tooltip = [
      `Checked route: ${page.route}`,
      `Output: ${page.output}`,
      `Source: ${page.source ?? 'Site program'}`,
      `Site: ${selected.folder.name}`,
      ...(page.url ? [`Configured URL: ${page.url}`] : []),
    ].join('\n')
    this.command = {
      title: 'Tola: Open Preview',
      command: 'tola.openPreview',
      arguments: [source, page.route, selected.folder.uri.toString()],
    }
  }
}
