import { expect, test } from '@playwright/test'
import {
  currentRevision,
  expectReloads,
  modifiedOutput,
  reloadDocument,
  sendRevision,
  serveDocument,
  type TolaRuntime,
  type TolaWindow,
} from '../../../support/reload.ts'

declare global {
  interface Window {
    canonicalRuntime?: TolaRuntime
    fragmentBeforeReady?: boolean
  }
}

test.describe('development revision protocol', () => {
  test('non-contiguous revision forces reload', async ({ page }) => {
    await serveDocument(page)

    await sendRevision(page, 'f'.repeat(64), 'e'.repeat(64), [modifiedOutput('unused.js')])
    await expectReloads(page, 1)
    await expect(await page.evaluate(() => (window as TolaWindow).Tola.revisionReloadRequired)).toBe(true)
    await expect(await currentRevision(page)).toBe('0'.repeat(64))

    await sendRevision(page, '0'.repeat(64), 'b'.repeat(64), [modifiedOutput('unused.js')])
    await expectReloads(page, 1)
    await expect(await currentRevision(page)).toBe('0'.repeat(64))
  })

  test('reconnect at another revision reloads', async ({ page }) => {
    await serveDocument(page)

    await page.evaluate(() => {
      ;(window as TolaWindow).Tola.handleMessage({
        type: 'connected',
        revision: 'a'.repeat(64),
        page_availability: 'present',
      })
    })
    await expectReloads(page, 1)
    await expect(await page.evaluate(() => (window as TolaWindow).Tola.revisionReloadRequired)).toBe(true)
  })

  test('changed generation reloads exactly once', async ({ page }) => {
    let generation = 'first-process'
    let documentRequests = 0
    let generationChecks = 0
    // Emulates a rejected handshake; the real process boundary is covered by the startup tests.
    await page.addInitScript(() => {
      Object.defineProperty(window, 'WebSocket', {
        value: class {
          onclose: ((event: CloseEvent) => void) | null = null
          constructor() {
            setTimeout(() => this.onclose?.(new CloseEvent('close')), 0)
          }
          close() {}
        },
      })
    })
    await page.route('https://restart.test/**', async (route) => {
      if (route.request().method() === 'HEAD') {
        generationChecks += 1
        if (generationChecks >= 2) generation = 'second-process'
        await route.fulfill({ status: 200, headers: { 'X-Tola-Reload-Generation': generation }, body: '' })
        return
      }
      documentRequests += 1
      await route.fulfill({
        status: 200,
        contentType: 'text/html',
        body: reloadDocument({
          body: `<main>${generation}</main>`,
          bootstrap: { port: 35729, generation },
        }),
      })
    })
    await page.goto('https://restart.test/index.html', { waitUntil: 'load' })
    await expect(page.locator('main')).toHaveText('second-process', { timeout: 10_000 })
    // Repeated handshakes at the same generation must not navigate again.
    await expect.poll(() => generationChecks, { timeout: 10_000 }).toBeGreaterThanOrEqual(4)
    expect(documentRequests).toBe(2)
  })

  test('reconnect discards in-flight stylesheet', async ({ page }) => {
    const connectionRevision = 'a'.repeat(64)
    const pendingRevision = 'b'.repeat(64)
    let documentRequests = 0
    let releaseStylesheet!: () => void
    const stylesheetReleased = new Promise<void>((resolve) => {
      releaseStylesheet = resolve
    })
    await page.route('https://queue.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/site.css') {
        const isPendingStylesheet = url.searchParams.get('tola-representation') === pendingRevision
        if (isPendingStylesheet) await stylesheetReleased
        await route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: `main { background-color: ${isPendingStylesheet ? 'blue' : 'red'}; }`,
        })
        return
      }
      if (url.pathname !== '/index.html') {
        await route.fulfill({ status: 404, body: 'Not found' })
        return
      }
      documentRequests += 1
      await route.fulfill({
        status: 200,
        contentType: 'text/html',
        body: reloadDocument({
          head: '<link rel="stylesheet" href="/site.css">',
          body: '<main>Current connection snapshot</main>',
          bootstrap: { revision: connectionRevision },
        }),
      })
    })
    try {
      await page.goto('https://queue.test/index.html', { waitUntil: 'load' })
      await expect(page.locator('main')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
      const stylesheetRequested = page.waitForRequest((request) => {
        const url = new URL(request.url())
        return url.pathname === '/site.css' && url.searchParams.get('tola-representation') === pendingRevision
      })
      await page.evaluate(({ connectionRevision, pendingRevision, change }) => {
        ;(window as TolaWindow).Tola.handleMessage({
          type: 'revision',
          page_availability: 'present',
          diff: { from: connectionRevision, to: pendingRevision, changes: [change] },
        })
      }, {
        connectionRevision,
        pendingRevision,
        change: modifiedOutput('site.css', 'asset', pendingRevision),
      })
      await stylesheetRequested
      await page.evaluate((current) => {
        ;(window as TolaWindow).Tola.handleMessage({
          type: 'connected',
          revision: current,
          page_availability: 'present',
        })
      }, connectionRevision)

      const navigation = page.waitForEvent('load', { timeout: 5_000 })
      releaseStylesheet()
      await navigation
      expect(documentRequests).toBe(2)
      await expect(page.locator('main')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
      expect(await currentRevision(page)).toBe(connectionRevision)
      await expect(page.locator('link[rel~="stylesheet"]')).toHaveCount(1)
      await expect(page.locator('link[data-tola-staging]')).toHaveCount(0)
    } finally {
      releaseStylesheet()
    }
  })

  test('early fragments preserve canonical page ownership', async ({ page }) => {
    const head = `<script>
      const original = document.querySelector('[data-tola-bootstrap]');
      window.canonicalRuntime = window.Tola;
      window.fragmentBeforeReady = window.Tola.timings.initialized === undefined;
      const foreign = { ...JSON.parse(original.dataset.tolaBootstrap), output: 'fragment.html', revision: '${
      'f'.repeat(64)
    }' };
      original.dataset.tolaBootstrap = JSON.stringify(foreign);
      const runtime = document.createElement('script');
      runtime.dataset.tolaRuntime = '';
      runtime.dataset.tolaBootstrap = JSON.stringify(foreign);
      runtime.textContent = original.textContent;
      document.head.appendChild(runtime);
      const fragment = document.createElement('script');
      fragment.textContent = "window.fragmentExecutions = (window.fragmentExecutions || 0) + 1;";
      document.head.appendChild(fragment);
    </script>`
    await page.route('https://early-fragment.test/**', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'text/html',
        body: reloadDocument({ head, body: '<main>Canonical page</main>' }),
      }))
    await page.goto('https://early-fragment.test/index.html', { waitUntil: 'load' })

    expect(
      await page.evaluate(() => ({
        sameRuntime: window.canonicalRuntime === (window as TolaWindow).Tola,
        beforeReady: window.fragmentBeforeReady,
        output: (window as TolaWindow).Tola.activeOutput,
        revision: (window as TolaWindow).Tola.revision,
        executions: window.fragmentExecutions,
      })),
    ).toEqual({
      sameRuntime: true,
      beforeReady: true,
      output: 'index.html',
      revision: '0'.repeat(64),
      executions: 1,
    })
    await expect(page.locator('main')).toHaveText('Canonical page')
  })

  test('availability change forces reload', async ({ page }) => {
    await serveDocument(page)

    await sendRevision(
      page,
      '0'.repeat(64),
      'b'.repeat(64),
      [modifiedOutput('unrelated.js')],
      'empty',
    )
    await expectReloads(page, 1)
    await expect(await page.evaluate(() => (window as TolaWindow).Tola.revisionReloadRequired)).toBe(true)
  })
})
