import { once } from 'node:events'
import * as fs from 'node:fs/promises'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import * as os from 'node:os'
import * as path from 'node:path'
import { fileURLToPath } from 'node:url'
import { runTests } from '@vscode/test-electron'

const require = createRequire(import.meta.url)

/** The VS Code release the integration host downloads: the oldest the extension declares, so
 * the suite proves its API floor rather than whatever stable shipped this week. */
const VSCODE_VERSION = '1.97.0'

/** The Playwright surface this driver uses: one CDP connection and the workbench notification. */
type DriverBrowser = { contexts(): DriverContext[]; close(): Promise<void> }

/** The Playwright module this driver loads, resolved from the extension's own dependencies. */
type DriverModule = { chromium: { connectOverCDP(endpoint: string): Promise<DriverBrowser> } }
type DriverContext = { pages(): DriverPage[] }
type DriverPage = {
  url(): string
  locator(selector: string): DriverLocator
  getByRole(role: string, options: { name: string | RegExp; exact?: boolean }): DriverLocator
  screenshot(options: { path: string }): Promise<unknown>
}
type DriverLocator = {
  filter(options: { has?: DriverLocator; hasText?: string | RegExp }): DriverLocator
  getByRole(role: string, options: { name: string | RegExp; exact?: boolean }): DriverLocator
  last(): DriverLocator
  waitFor(options: { state: string; timeout: number }): Promise<void>
  innerText(): Promise<string>
  hover(): Promise<void>
  click(): Promise<void>
}

type WorkbenchAction = {
  button?: string
  dismiss?: boolean
  contains?: string
  error?: boolean
  status?: string
  pages?: string
  page?: string
  pagesMessage?: string
  capture?: boolean
}

/** One shell word carrying `value` literally, so the generated script runs the named runtime. */
function quoted(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`
}

async function main(): Promise<void> {
  const directory = import.meta.dirname ?? path.dirname(fileURLToPath(import.meta.url))
  const repository = path.resolve(directory, '../../..')
  const executable = process.env.TOLA_TEST_BINARY ||
    path.join(repository, 'target/debug', process.platform === 'win32' ? 'tola.exe' : 'tola')
  await fs.access(executable)
  const { chromium } = require('playwright') as DriverModule
  const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'tola-vscode-'))
  const userData = path.join(temporary, 'user-data')
  // The suite's helper scripts — the service proxy and the publication hooks — run under this
  // driver's own runtime, which needs the permission flag they would otherwise go without.
  const helperRuntime = path.join(temporary, 'helper-runtime')
  await fs.writeFile(
    helperRuntime,
    `#!/bin/sh\nexec ${quoted(process.execPath)}${'deno' in process.versions ? ' run -A' : ''} "$@"\n`,
    { mode: 0o755 },
  )
  let browser: DriverBrowser | undefined
  const driver = createServer(async (request, response) => {
    try {
      let body = ''
      for await (const chunk of request) body += chunk
      const action = JSON.parse(body) as WorkbenchAction
      if (!browser) {
        const [port] = (await fs.readFile(path.join(userData, 'DevToolsActivePort'), 'utf8')).split('\n')
        if (!port) throw new Error('The workbench did not publish its debugging port')
        browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`)
      }
      const page = browser.contexts().flatMap((context) => context.pages())
        .find((candidate) => candidate.url().includes('workbench.html'))
      if (!page) throw new Error('The VS Code workbench is not attached to the test driver')
      if (action.pages !== undefined || action.page !== undefined || action.pagesMessage !== undefined) {
        const target = action.page !== undefined
          ? page.getByRole('treeitem', { name: action.page }).last()
          : action.pagesMessage !== undefined
          ? page.locator('.monaco-pane-view .pane').filter({ hasText: action.pagesMessage }).last()
          : page.getByRole('tree', { name: 'Tola Pages' }).filter({ hasText: action.pages ?? '' })
        await target.waitFor({ state: 'visible', timeout: 15_000 })
        const text = await target.innerText()
        if (action.page !== undefined) await target.click()
        const capture = action.capture
          ? path.join(os.tmpdir(), `tola-vscode-pages-${process.pid}.png`)
          : undefined
        if (capture) await page.screenshot({ path: capture })
        response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ text, capture }))
        return
      }
      if (action.status !== undefined) {
        const status = page.locator('.statusbar-item').filter({ hasText: new RegExp(action.status) }).last()
        await status.waitFor({ state: 'visible', timeout: 15_000 })
        const capture = action.capture
          ? path.join(os.tmpdir(), `tola-vscode-status-${process.pid}.png`)
          : undefined
        if (capture) await page.screenshot({ path: capture })
        response.writeHead(200, { 'content-type': 'application/json' })
          .end(JSON.stringify({ text: await status.innerText(), capture }))
        return
      }
      const notification = page.locator('.notification-list-item').filter(
        action.button
          ? { has: page.getByRole('button', { name: action.button, exact: true }) }
          : action.error
          ? { has: page.locator('.codicon-error') }
          : { hasText: action.contains ?? '' },
      ).last()
      await notification.waitFor({ state: 'visible', timeout: 15_000 })
      const text = await notification.innerText()
      if (action.dismiss) {
        await notification.hover()
        await notification.getByRole('button', { name: /Clear Notification/i }).click()
      } else if (action.button) {
        await notification.getByRole('button', { name: action.button, exact: true }).click()
      }
      response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ text }))
    } catch (error) {
      response.writeHead(500, { 'content-type': 'text/plain' }).end(String(error))
    }
  })
  driver.listen(0, '127.0.0.1')
  await once(driver, 'listening')
  const address = driver.address()
  if (!address || typeof address === 'string') throw new Error('The test driver did not bind a TCP port')
  try {
    // Keeping the first folder prevents workspace-folder changes from restarting the test host.
    const anchor = path.join(temporary, 'ordinary-typst')
    await fs.mkdir(path.join(anchor, '.vscode'), { recursive: true })
    await fs.writeFile(path.join(anchor, 'ordinary.typ'), '= Ordinary Typst\n')
    await fs.writeFile(
      path.join(anchor, '.vscode/settings.json'),
      JSON.stringify({
        'tola.serverPath': '/must-not-start-in-non-tola-workspace',
        'tola.enabled': false,
      }),
    )
    const workspace = path.join(temporary, 'test.code-workspace')
    await fs.writeFile(workspace, JSON.stringify({ folders: [{ path: anchor }] }))
    await runTests({
      cachePath: path.join(directory, '..', '.vscode-test'),
      extensionDevelopmentPath: path.resolve(directory, '..'),
      extensionTestsPath: path.resolve(directory, '../dist/test.cjs'),
      extensionTestsEnv: {
        TOLA_TEST_ROOT: temporary,
        TOLA_TEST_BINARY: executable,
        TOLA_TEST_NODE: helperRuntime,
        TOLA_TEST_DRIVER: `http://127.0.0.1:${address.port}`,
        TOLA_TEST_FILTER: process.env.TOLA_TEST_FILTER ?? '',
      },
      launchArgs: [
        workspace,
        '--user-data-dir',
        userData,
        '--extensions-dir',
        path.join(temporary, 'extensions'),
        '--shared-data-dir',
        path.join(temporary, 'shared-data'),
        '--remote-debugging-port=0',
        '--disable-extensions',
        '--disable-workspace-trust',
        '--skip-welcome',
        '--skip-release-notes',
      ],
      ...(process.env.VSCODE_EXECUTABLE_PATH
        ? { vscodeExecutablePath: process.env.VSCODE_EXECUTABLE_PATH }
        : { version: VSCODE_VERSION }),
    })
  } finally {
    if (browser) await browser.close()
    driver.closeAllConnections()
    const closed = Promise.withResolvers<void>()
    driver.close(() => closed.resolve())
    await closed.promise
    await fs.rm(temporary, { recursive: true, force: true })
  }
}

main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
