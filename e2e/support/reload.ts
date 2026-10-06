import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { expect, type Page } from '@playwright/test'

const ROOT = resolve(__dirname, '../..')

export const RELOAD_SOURCE = readFileSync(resolve(ROOT, 'src/embed/dev/hotreload.js'), 'utf8')
  .replace('__TOLA_DEV_STATUS_CSS__', '')

type OutputKind =
  | 'html-document'
  | 'pdf-document'
  | 'png-document'
  | 'svg-document'
  | 'asset'
type OutputEntry = { path: string; kind: OutputKind; representation: string; size: number }
type OutputChange =
  | { operation: 'added' | 'removed'; output: OutputEntry }
  | { operation: 'modified'; output: { before: OutputEntry; after: OutputEntry } }
type PageAvailability = 'empty' | 'present'

/** The development status the indicator shows. */
export type TolaStatus = {
  rebuilding: boolean
  errors: number
  warnings: number
}

/** Mirrors the surface `src/embed/dev/hotreload.js` installs on a served page. */
export type TolaRuntime = {
  revision: string | null
  revisionReloadRequired: boolean
  activeOutput: string | null
  revisionUpdates: Promise<void>
  awaitingPublication: boolean
  timings: Record<string, number>
  ws: WebSocket | null
  status: TolaStatus
  handleMessage(message: unknown): void
  requestReload(): void
  saveReloadState(): void
}

export type TolaWindow = typeof window & { reloads: number; Tola: TolaRuntime }

export type ReloadBootstrap = {
  port?: number
  session?: string
  revision?: string | null
  output?: string | null
  page_availability?: PageAvailability | null
  mount?: string
  generation?: string
}

export type ReloadDocumentOptions = {
  head?: string
  body?: string
  bootstrap?: ReloadBootstrap
}

function bootstrapMetadata(bootstrap: ReloadBootstrap = {}) {
  return {
    port: bootstrap.port ?? 0,
    session: bootstrap.session ?? 'test-session',
    revision: bootstrap.revision === undefined ? '0'.repeat(64) : bootstrap.revision,
    output: bootstrap.output === undefined ? 'index.html' : bootstrap.output,
    page_availability: bootstrap.page_availability === undefined ? 'present' : bootstrap.page_availability,
    mount: bootstrap.mount ?? '',
    generation: bootstrap.generation ?? 'test-generation',
  }
}

export function reloadDocument({
  head = '',
  body = '',
  bootstrap = {},
}: ReloadDocumentOptions = {}): string {
  const metadata = JSON.stringify(bootstrapMetadata(bootstrap))
    .replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
  const runtime = `<script data-tola-runtime data-tola-bootstrap="${metadata}">${RELOAD_SOURCE}</script>`
  return `<!doctype html><html><head>${runtime}${head}</head><body>${body}</body></html>`
}

export function modifiedOutput(
  path: string,
  kind: OutputKind = 'asset',
  representation = 'b'.repeat(64),
): OutputChange {
  return {
    operation: 'modified',
    output: {
      before: { path, kind, representation: 'a'.repeat(64), size: 1 },
      after: { path, kind, representation, size: 1 },
    },
  }
}

export function addedOutput(
  path: string,
  kind: OutputKind = 'asset',
  representation = 'b'.repeat(64),
): OutputChange {
  return {
    operation: 'added',
    output: { path, kind, representation, size: 1 },
  }
}

/** Replaces the runtime's navigational reload with a counter the test can observe. */
export async function recordReloads(page: Page): Promise<void> {
  await page.evaluate(() => {
    const runtime = window as TolaWindow
    runtime.reloads = 0
    runtime.Tola.requestReload = () => {
      runtime.reloads += 1
    }
  })
}

export function currentRevision(page: Page): Promise<string | null> {
  return page.evaluate(() => (window as TolaWindow).Tola.revision)
}

/** Waits until the served page's development socket is open. */
export async function waitForOpenSocket(page: Page): Promise<void> {
  await page.waitForFunction(() => (window as TolaWindow).Tola?.ws?.readyState === WebSocket.OPEN)
}

export function runtimeState(page: Page): Promise<{ reloads: number; revision: string | null }> {
  return page.evaluate(() => {
    const runtime = window as TolaWindow
    return { reloads: runtime.reloads, revision: runtime.Tola.revision }
  })
}

export async function expectReloads(page: Page, count: number): Promise<void> {
  expect(await page.evaluate(() => (window as TolaWindow).reloads)).toBe(count)
}

export async function installReload(page: Page, output = 'index.html', pathPrefix = '') {
  await page.evaluate(({ source, metadata }) => {
    const script = document.createElement('script')
    script.dataset.tolaRuntime = ''
    script.dataset.tolaBootstrap = JSON.stringify(metadata)
    script.textContent = source
    document.head.appendChild(script)
  }, { source: RELOAD_SOURCE, metadata: bootstrapMetadata({ output, mount: pathPrefix }) })
  await page.waitForFunction(() => (window as TolaWindow).Tola?.timings.initialized !== undefined)
  await recordReloads(page)
}

/** Serves `body` for every request on `url`'s origin, then loads the runtime on `url` itself. */
export async function serveDocument(
  page: Page,
  {
    body = '<!doctype html><html><body><main>stable</main></body></html>',
    url = 'https://tola.test/index.html',
    status = 200,
    output = 'index.html',
    pathPrefix = '',
  } = {},
): Promise<void> {
  await page.route(
    `${new URL(url).origin}/**`,
    (route) => route.fulfill({ status, contentType: 'text/html', body }),
  )
  await page.goto(url)
  await installReload(page, output, pathPrefix)
}

export async function sendNextRevision(page: Page, changes: OutputChange[]) {
  const from = await page.evaluate(() => (window as TolaWindow).Tola.revision)
  if (from === null) throw new Error('The page has no revision to advance from')
  const to = from === 'b'.repeat(64) ? 'c'.repeat(64) : 'b'.repeat(64)
  await sendRevision(page, from, to, changes)
}

export async function sendRevision(
  page: Page,
  from: string,
  to: string,
  changes: OutputChange[],
  pageAvailability: PageAvailability = 'present',
) {
  await page.evaluate(({ from, to, changes, pageAvailability }) => {
    const tola = (window as TolaWindow).Tola
    tola.handleMessage({
      type: 'revision',
      page_availability: pageAvailability,
      diff: { from, to, changes },
    })
    return tola.revisionUpdates
  }, { from, to, changes, pageAvailability })
}
