import { expect, test } from '@playwright/test'
import {
  addedOutput,
  currentRevision,
  expectReloads,
  installReload,
  modifiedOutput,
  recordReloads,
  reloadDocument,
  runtimeState,
  sendNextRevision,
  serveDocument,
} from '../../../support/reload.ts'

declare global {
  interface Window {
    requestedSettings?: Promise<unknown>
  }
}

test('scripted HTML edit requests navigation', async ({ page }) => {
  let text = 'stable'
  await page.route(
    'https://scripts.test/**',
    (route) =>
      route.fulfill(
        route.request().url().endsWith('.js')
          ? { status: 200, contentType: 'text/javascript', body: 'window.applicationLoaded = true;' }
          : {
            status: 200,
            contentType: 'text/html',
            body:
              `<!doctype html><html><head><script src="/app.js"></script></head><body><p id="text">${text}</p></body></html>`,
          },
      ),
  )
  await page.goto('https://scripts.test/index.html', { waitUntil: 'load' })
  await installReload(page)

  text = 'changed'
  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('#text')).toHaveText('stable')
  expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
})

test('pending request survives output change', async ({ page }) => {
  let requestStarted!: () => void
  const started = new Promise<void>((resolve) => {
    requestStarted = resolve
  })
  let releaseResponse!: () => void
  const responseReleased = new Promise<void>((resolve) => {
    releaseResponse = resolve
  })
  await page.route('https://pending.test/**', async (route) => {
    if (new URL(route.request().url()).pathname === '/settings.json') {
      requestStarted()
      await responseReleased
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"value":1}' })
      return
    }
    await route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: reloadDocument({
        body:
          `<script>window.requestedSettings = fetch('/settings.json').then(response => response.json());</script>`,
      }),
    })
  })
  try {
    await page.goto('https://pending.test/index.html', { waitUntil: 'domcontentloaded' })
    await started
    await recordReloads(page)
    expect(await page.evaluate(() => performance.getEntriesByName(`${location.origin}/settings.json`)))
      .toHaveLength(0)

    await sendNextRevision(page, [modifiedOutput('settings.json')])

    await expectReloads(page, 0)
    expect(await currentRevision(page)).toBe('b'.repeat(64))
  } finally {
    releaseResponse()
    await page.evaluate(() => window.requestedSettings)
  }
})

for (
  const { address, url, mount, output } of [
    { address: 'root', url: '/', mount: '', output: 'index.html' },
    { address: 'post directory', url: '/post/', mount: '', output: 'post/index.html' },
    { address: 'post file', url: '/post', mount: '', output: 'post' },
    { address: 'download', url: '/download', mount: '', output: 'download' },
    { address: 'download directory', url: '/download/', mount: '', output: 'download/index.html' },
    { address: 'mount root', url: '/docs/', mount: 'docs', output: 'index.html' },
    { address: 'mounted post', url: '/docs/post/', mount: 'docs', output: 'post/index.html' },
  ]
) {
  test(`output added at ${address} reloads`, async ({ page }) => {
    await serveDocument(page, { url: `https://routes.test${url}`, output, pathPrefix: mount })

    await sendNextRevision(page, [addedOutput(output, output.endsWith('.html') ? 'html-document' : 'asset')])

    await expectReloads(page, 1)
  })
}

test('unrelated output leaves page unchanged', async ({ page }) => {
  await serveDocument(page, {
    body: '<!doctype html><html><body><a href="/another/">Another page</a></body></html>',
  })

  await sendNextRevision(page, [modifiedOutput('unused.js')])

  await expectReloads(page, 0)
  expect(await currentRevision(page)).toBe('b'.repeat(64))
})

test('navigation links do not force reload', async ({ page }) => {
  await serveDocument(page, {
    body:
      '<!doctype html><html><body><nav><a href="/another/">Another page</a><a href="/guide.pdf">Guide</a></nav><map name="sitemap"><area href="/another/" alt="Another page"></map></body></html>',
  })

  await sendNextRevision(page, [
    modifiedOutput('another/index.html', 'html-document'),
    modifiedOutput('guide.pdf'),
  ])

  await expectReloads(page, 0)
  expect(await currentRevision(page)).toBe('b'.repeat(64))
})

test('linked stylesheet updates in place', async ({ page }) => {
  await page.route('https://linked-stylesheet.test/**', (route) => {
    const url = new URL(route.request().url())
    if (url.pathname === '/site.css') {
      return route.fulfill({
        status: 200,
        contentType: 'text/css',
        body: `body { color: ${url.searchParams.has('tola-representation') ? 'blue' : 'red'}; }`,
      })
    }
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><a href="/site.css">Stylesheet</a></body></html>',
    })
  })
  await page.goto('https://linked-stylesheet.test/index.html')
  await installReload(page)

  await sendNextRevision(page, [modifiedOutput('site.css')])

  await expect(page.locator('link[rel~=stylesheet]')).toHaveAttribute('href', /tola-representation=b{64}/)
  await expect(page.locator('body')).toHaveCSS('color', 'rgb(0, 0, 255)')
  await expectReloads(page, 0)
  expect(await currentRevision(page)).toBe('b'.repeat(64))
})

test('rendered asset forces full reload', async ({ page }) => {
  await serveDocument(page, {
    body: '<!doctype html><html><body><svg><use href="/icons.svg#search"></use></svg></body></html>',
  })

  await sendNextRevision(page, [modifiedOutput('icons.svg')])

  await expectReloads(page, 1)
  expect(await currentRevision(page)).toBe('0'.repeat(64))
})

test('malformed mount URL does not reload', async ({ page }) => {
  await serveDocument(page, {
    body: '<!doctype html><html><body><img src="/docs//"></body></html>',
    url: 'https://routes.test/docs/current.html',
    output: 'current.html',
    pathPrefix: 'docs',
  })

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expectReloads(page, 0)
})

test('inline handler page ignores change', async ({ page }) => {
  await serveDocument(page, {
    body:
      '<!doctype html><html><body><button onclick="fetch(\'/settings.json\')">Load settings</button></body></html>',
  })

  await sendNextRevision(page, [modifiedOutput('settings.json')])

  await expectReloads(page, 0)
  expect(await currentRevision(page)).toBe('b'.repeat(64))
})

for (
  const { file, consumer } of [
    { file: 'dependency.js', consumer: 'script' },
    { file: 'settings.json', consumer: 'json' },
  ]
) {
  test(`cleared timings reload for ${consumer}`, async ({ page }) => {
    await page.route('https://history.test/**', (route) => {
      const path = new URL(route.request().url()).pathname
      if (path.endsWith('.js')) {
        return route.fulfill({
          status: 200,
          contentType: 'text/javascript',
          body: path === '/entry.js' ? 'export { value } from "./dependency.js";' : 'export const value = 1;',
        })
      }
      if (path.endsWith('.json')) {
        return route.fulfill({ status: 200, contentType: 'application/json', body: '{"value":1}' })
      }
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body: '<!doctype html><html><body>Document</body></html>',
      })
    })
    await page.goto('https://history.test/index.html')
    await installReload(page)
    await page.evaluate(async () => {
      await import(`${location.origin}/entry.js`)
      await fetch('/settings.json').then((response) => response.json())
      performance.clearResourceTimings()
    })

    await sendNextRevision(page, [modifiedOutput(file)])

    await expectReloads(page, 1)
    expect(await currentRevision(page)).toBe('0'.repeat(64))
  })
}

test('initial timing overflow forces reload', async ({ page }) => {
  await page.route('https://initial-capacity.test/**', (route) => {
    const url = new URL(route.request().url())
    if (url.pathname === '/pixel.svg') {
      return route.fulfill({
        status: 200,
        contentType: 'image/svg+xml',
        body: '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>',
      })
    }
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: `<!doctype html><html><body>${
        Array.from({ length: 250 }, (_, index) => `<img src="/pixel.svg?request=${index}">`).join('')
      }</body></html>`,
    })
  })
  await page.goto('https://initial-capacity.test/index.html', { waitUntil: 'load' })
  expect(await page.evaluate(() => performance.getEntriesByType('resource').length)).toBeGreaterThanOrEqual(
    250,
  )
  await installReload(page)
  await sendNextRevision(page, [modifiedOutput('unobserved.json')])
  await expectReloads(page, 1)
  expect(await currentRevision(page)).toBe('0'.repeat(64))
})
