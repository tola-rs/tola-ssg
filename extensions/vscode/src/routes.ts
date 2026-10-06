import type * as vscode from 'vscode'
import { type LanguageClient, RequestType } from 'vscode-languageclient/node'

/** A route realized by a source check, not evidence of development publication. */
export interface Route {
  readonly output: string
  readonly route: string
  readonly url?: string
}

export interface SiteRoute extends Route {
  readonly source?: string
}

const siteRoutesRequest = new RequestType<Record<string, never>, { routes: SiteRoute[] }, void>('tola/routes')

const routeRequest = new RequestType<{ uri: string }, { routes: Route[] }, void>('tola/route')

/** The routes a source's own check realizes, including text the editor has not saved. */
export function checkedRoutes(
  client: LanguageClient,
  uri: vscode.Uri,
  token: vscode.CancellationToken,
): Promise<Route[]> {
  return client.sendRequest(routeRequest, { uri: uri.toString() }, token).then((reply) => reply.routes)
}

export function checkedSiteRoutes(
  client: LanguageClient,
  token: vscode.CancellationToken,
): Promise<SiteRoute[]> {
  return client.sendRequest(siteRoutesRequest, {}, token).then((reply) => reply.routes)
}
