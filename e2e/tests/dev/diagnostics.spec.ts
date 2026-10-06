import { expect, type Page, type WebSocket } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { readSessionRecords, servingUrl } from '../../support/log.ts'
import { startCommand } from '../../support/process.ts'
import { test } from '../../support/server.ts'
import { installReload, type TolaStatus, type TolaWindow } from '../../support/reload.ts'
import { writeMinimalSite, writeSiteProgram } from '../../support/site.ts'

/** One status message the development server pushes to a served page. */
type DevStatus = TolaStatus & { type: string }

test('first build reports no diagnostics to the browser', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: null,
    // The first build warns without a `404.html` document, so this site has one.
    beforeStart: (root) =>
      writeSiteProgram(
        root,
        `#document("index.html", format: "html")[Home]
#document("404.html", format: "html")[Missing route]
`,
      ),
  })
  const status = await statusAfterLoad(page, site.url)
  expect(status).toMatchObject({ errors: 0, warnings: 0 })
})

test('missing not found document reports one warning', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: null,
    beforeStart: (root) => writeSiteProgram(root, '#document("index.html", format: "html")[Home]\n'),
  })
  const status = await statusAfterLoad(page, site.url)
  expect(status).toMatchObject({ errors: 0, warnings: 1 })
})

test('recovery compacts diagnostic progress', async ({ sites }) => {
  const errors = ['#let broken1 = ', '#let broken2 = ', '#let broken3 = ', '#let broken4 = ']
  const failedProgram = `${errors.join('\n')}\n#document("index.html")[Rejected]\n`
  const site = await sites.dev({
    initialContent: null,
    beforeStart: async (root) => {
      await writeFile(join(root, 'tola.toml'), '[diagnostics]\nmax_errors = 2\n')
      await writeSiteProgram(root, failedProgram)
    },
  })
  await expect.poll(async () =>
    (await readSessionRecords(site.root)).filter((record) =>
      record.fields.kind === 'diagnostic' && record.fields.code === 'typst.compile'
    ).length
  ).toBe(4)
  const diagnostics = (await readSessionRecords(site.root)).filter((record) =>
    record.fields.kind === 'diagnostic' && record.fields.code === 'typst.compile'
  )
  expect(diagnostics.map((record) =>
    (record.fields as typeof record.fields & {
      diagnostic: { severity: string; location: { path: string; line: number; column: number } }
    }).diagnostic
  )).toEqual(errors.map((source, index) =>
    expect.objectContaining({
      severity: 'error',
      location: expect.objectContaining({
        path: 'site.typ',
        line: index + 1,
        column: source.length,
      }),
    })
  ))
  const failedTranscript = site.stderr()
  expect(failedTranscript.match(/error\[typst\.compile\]/g)).toHaveLength(2)
  await writeSiteProgram(
    site.root,
    '#document("index.html")[Recovered]\n#document("404.html")[Missing]\n',
  )
  await expect.poll(async () =>
    (await readSessionRecords(site.root)).filter((record) => record.fields.kind === 'resolved').length
  ).toBe(4)
  await expect.poll(() => site.stderr().slice(failedTranscript.length).trim())
    .toMatch(/\bbuild succeeded\b/i)
  const recovery = site.stderr().slice(failedTranscript.length).trim().split('\n')
  expect(recovery).toHaveLength(1)
  expect(recovery[0]).toMatch(/\bbuild succeeded\b/i)
  const published = await fetch(site.url)
  expect(published.status).toBe(200)
  expect(await published.text()).toContain('Recovered')
})

test('quiet dev retains diagnostics', async ({ binary, directory: root }) => {
  await writeMinimalSite(root, {
    program: '#document("index.html")[Quiet publication]\n',
  })
  const server = startCommand(binary, [
    'dev',
    '--quiet',
    '--interface',
    '127.0.0.1',
    '--port',
    '0',
    '--log-file',
    '.tola/logs/session.jsonl',
    '--color',
    'never',
  ], root)
  try {
    let url: string | undefined
    await expect.poll(async () => (url = await servingUrl(root))).toBeDefined()
    const response = await fetch(url!)
    expect(response.status).toBe(200)
    expect(await response.text()).toContain('Quiet publication')
    expect(server.stderr()).toContain('warning[site.not_found_missing]')
    const startup = (await readSessionRecords(root)).find((record) => record.fields.kind === 'summary')
    const summary = (startup?.fields as { message?: string } | undefined)?.message
    expect(summary).toEqual(expect.any(String))
    expect(server.stderr()).not.toContain(summary!)
    expect(server.stderr()).not.toContain(url!)
    await writeSiteProgram(root, '#document("index.html")[#unknown_quiet_edit]\n')
    await expect.poll(server.stderr).toContain('error[typst.compile]')
    expect(server.stderr()).toContain('still serving')
    expect(await (await fetch(url!)).text()).toContain('Quiet publication')
    const failureTranscript = server.stderr()
    await writeSiteProgram(root, '#document("index.html")[Quiet recovery]\n')
    await expect.poll(async () => {
      const records = await readSessionRecords(root)
      return records.some((record) =>
        record.fields.kind === 'resolved' && record.fields.code === 'typst.compile'
      )
    }).toBe(true)
    let recoverySummary: string | undefined
    await expect.poll(async () => {
      const records = await readSessionRecords(root)
      const resolved = records.findIndex((record) =>
        record.fields.kind === 'resolved' && record.fields.code === 'typst.compile'
      )
      const recovered = records.slice(resolved).find((record) => record.fields.kind === 'summary')
      recoverySummary = (recovered?.fields as { message?: string } | undefined)?.message
      return recoverySummary
    }).toEqual(expect.any(String))
    expect(await (await fetch(url!)).text()).toContain('Quiet recovery')
    await server.command.terminate()
    expect(server.stderr().slice(failureTranscript.length)).not.toContain(recoverySummary!)
  } finally {
    await server.command.terminate()
  }
})

/** The first status the development server pushes after `url` loads. */
async function statusAfterLoad(page: Page, url: string): Promise<DevStatus> {
  const frames: string[] = []
  const sockets = new Set<WebSocket>()
  const received = (frame: { payload: string | Buffer }) => {
    frames.push(frame.payload.toString())
  }
  const connected = (socket: WebSocket) => {
    sockets.add(socket)
    socket.on('framereceived', received)
  }
  const status = () =>
    frames
      .map((frame) => JSON.parse(frame) as DevStatus)
      .find((message) => message.type === 'status')
  page.on('websocket', connected)
  try {
    await page.goto(url, { waitUntil: 'load' })
    await expect.poll(status).toBeDefined()
    return status()!
  } finally {
    page.removeListener('websocket', connected)
    for (const socket of sockets) socket.removeListener('framereceived', received)
  }
}

test('indicator reflects the pushed counts', async ({ page }) => {
  await statusPage(page)
  await submitStatus(page, { rebuilding: false, errors: 2, warnings: 1 })

  const indicator = page.locator('#tola-dev-status')
  await expect(indicator).toBeVisible()
  await expect(indicator.locator('.tola-dev-status-count[data-severity="error"] .tola-dev-status-value'))
    .toHaveText('2')
  await expect(indicator.locator('.tola-dev-status-count[data-severity="warning"] .tola-dev-status-value'))
    .toHaveText('1')

  await submitStatus(page, { rebuilding: false, errors: 0, warnings: 0 })
  await expect(indicator).not.toBeVisible()
})

test('rebuild indicator waits out the reveal delay', async ({ page }) => {
  await statusPage(page)
  await submitStatus(page, { rebuilding: true, errors: 0, warnings: 0 })
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()

  await expect(page.locator('#tola-dev-status')).toBeVisible()
  await expect(page.locator('.tola-dev-status-label')).toHaveText('Rebuilding site')

  await submitStatus(page, { rebuilding: false, errors: 0, warnings: 0 })
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()
})

async function statusPage(page: Page) {
  await page.route('https://dev-status.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        '<!doctype html><html><body><main><button id="page-action">Page action</button></main></body></html>',
    }))
  await page.goto('https://dev-status.test/index.html', { waitUntil: 'load' })
  await installReload(page)
}

async function submitStatus(page: Page, status: TolaStatus) {
  await page.evaluate((next) => {
    ;(window as TolaWindow).Tola.handleMessage({ type: 'status', ...next })
  }, status)
}
