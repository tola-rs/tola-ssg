import { expect, type Page, test } from '@playwright/test'
import {
  currentRevision,
  expectReloads,
  installReload,
  modifiedOutput,
  runtimeState,
  sendNextRevision,
  type TolaWindow,
} from '../../../support/reload.ts'
import { ONE_PIXEL_PNG } from '../../../support/media.ts'

declare global {
  interface Window {
    reportResourceUrls?: (urls: string[]) => void
    imageDecodeStarted?: Promise<void>
    finishImageDecode?: (failed: boolean) => void
    resourceApplication?: object
  }
}

/**
 * Loads `body` at `origin/index.html` with every other path answered by the shared pixel image,
 * then installs the runtime.
 */
async function serveImagePage(page: Page, origin: string, body: string): Promise<void> {
  await page.route(
    `${origin}/**`,
    (route) =>
      new URL(route.request().url()).pathname === '/index.html'
        ? route.fulfill({ status: 200, contentType: 'text/html', body })
        : route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG }),
  )
  await page.goto(`${origin}/index.html`, { waitUntil: 'load' })
  await installReload(page)
}

test.describe('development resource updates', () => {
  for (const consumer of ['mixed image data', 'late image data', 'late inline image']) {
    test(`${consumer} requires asset owner`, async ({ page }) => {
      const { promise: released, resolve: releaseStylesheet } = Promise.withResolvers<void>()
      await page.route('https://mixed-resources.test/**', async (route) => {
        const url = new URL(route.request().url())
        if (url.pathname === '/site.css') {
          if (url.searchParams.has('tola-representation')) await released
          return route.fulfill({
            status: 200,
            contentType: 'text/css',
            body: `body { color: ${url.searchParams.has('tola-representation') ? 'blue' : 'red'}; }`,
          })
        }
        if (url.pathname === '/cover.png') {
          return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
        }
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><img id="cover" src="/cover.png"><script>window.resourceApplication = {};</script></body></html>',
        })
      })
      await page.goto('https://mixed-resources.test/index.html', { waitUntil: 'load' })
      await installReload(page)
      const fetchImage = () =>
        page.evaluate(async () => {
          const observed = new Promise<void>((resolve) => {
            const observer = new PerformanceObserver((entries) => {
              if (entries.getEntries().some((entry) => entry.name.endsWith('/cover.png'))) {
                observer.disconnect()
                resolve()
              }
            })
            observer.observe({ type: 'resource' })
          })
          await fetch('/cover.png').then((response) => response.arrayBuffer())
          await observed
        })
      if (consumer === 'mixed image data') await fetchImage()
      const requested = consumer === 'mixed image data'
        ? null
        : page.waitForRequest((request) => request.url().includes('/site.css?tola-representation'))
      const update = sendNextRevision(page, [modifiedOutput('site.css'), modifiedOutput('cover.png')])
      try {
        if (requested) {
          await requested
          if (consumer === 'late image data') await fetchImage()
          else {await page.evaluate(() => {
              const style = document.createElement('style')
              style.textContent = 'body { background-image: url(/cover.png); }'
              document.head.appendChild(style)
            })}
        }
      } finally {
        releaseStylesheet()
        await update
      }
      await expect(page.locator('body')).toHaveCSS('color', 'rgb(255, 0, 0)')
      await expect(page.locator('#cover')).toHaveAttribute('src', '/cover.png')
      await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
      expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
    })
  }

  for (const change of ['new font', 'unloaded font', 'image capacity']) {
    test(`${change} refuses stylesheet commit`, async ({ page }) => {
      await page.route('https://bounded-styles.test/**', (route) => {
        const url = new URL(route.request().url())
        if (url.pathname === '/site.css') {
          return route.fulfill({
            status: 200,
            contentType: 'text/css',
            body: !url.searchParams.has('tola-representation')
              ? `${
                change === 'unloaded font'
                  ? '@font-face { font-family: new-font; src: url(/new.woff2); }'
                  : ''
              } body { color: red; }`
              : change !== 'image capacity'
              ? '@font-face { font-family: new-font; src: url(/new.woff2); } body { color: blue; font-family: new-font; }'
              : `body { color: blue; background-image: ${
                Array.from({ length: 129 }, (_, index) => `url(/image-${index}.png)`).join(',')
              }; }`,
          })
        }
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><input id="search" value="seed"></body></html>',
        })
      })
      await page.goto('https://bounded-styles.test/index.html', { waitUntil: 'load' })
      await installReload(page)
      await page.locator('#search').fill('retained input')

      await sendNextRevision(page, [modifiedOutput('site.css')])

      await expect(page.locator('body')).toHaveCSS('color', 'rgb(255, 0, 0)')
      await expect(page.locator('#search')).toHaveValue('retained input')
      await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
      expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
    })
  }

  for (const behavior of ['scriptless', 'scripted']) {
    test(`${behavior} class edit awaits cold background`, async ({ page }) => {
      let classes = 'first'
      const { promise: released, resolve: releaseImage } = Promise.withResolvers<void>()
      await page.route('https://class-background.test/**', async (route) => {
        const request = route.request()
        const url = new URL(request.url())
        if (url.pathname === '/site.css') {
          return route.fulfill({
            status: 200,
            contentType: 'text/css',
            body: '.first { background: red; } .second { background: blue url(/new.svg); }',
          })
        }
        if (url.pathname === '/new.svg') {
          await released
          return route.fulfill({
            status: 200,
            contentType: 'image/svg+xml',
            body: '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>',
          })
        }
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            `<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><section id="panel" class="${
              request.headers()['x-tola-revision'] === '0'.repeat(64) ? 'first' : classes
            }"><input id="search" value="seed"></section>${
              behavior === 'scripted' ? '<script>window.resourceApplication = {};</script>' : ''
            }</body></html>`,
        })
      })
      await page.goto('https://class-background.test/index.html', { waitUntil: 'load' })
      await installReload(page)
      await page.locator('#search').fill('retained input')
      classes = 'second'
      const requested = page.waitForRequest((request) => new URL(request.url()).pathname === '/new.svg')
      const update = sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])
      try {
        await Promise.race([
          requested,
          update.then(() => {
            throw new Error('Class update completed before its background image was ready')
          }),
        ])
        await expect(page.locator('#panel')).toHaveClass('first')
        await expect(page.locator('#panel')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
        expect(await runtimeState(page)).toEqual({ reloads: 0, revision: '0'.repeat(64) })
      } finally {
        releaseImage()
        await update
      }
      await expect(page.locator('#panel')).toHaveClass('second')
      await expect(page.locator('#panel')).toHaveCSS('background-color', 'rgb(0, 0, 255)')
      await expect(page.locator('#search')).toHaveValue('retained input')
      expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
    })
  }

  for (const change of ['stylesheet source', 'image source', 'stylesheet removal']) {
    test(`${change} rejects prepared update`, async ({ page }) => {
      const { promise: released, resolve: releaseStylesheet } = Promise.withResolvers<void>()
      await page.route('https://resource-owner.test/**', async (route) => {
        const url = new URL(route.request().url())
        if (url.pathname.endsWith('.css')) {
          if (url.searchParams.has('tola-representation')) await released
          return route.fulfill({ status: 200, contentType: 'text/css', body: 'body { color: red; }' })
        }
        if (url.pathname.endsWith('.png')) {
          return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
        }
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><img id="cover" src="/cover.png"><script>window.resourceApplication = {};</script></body></html>',
        })
      })
      await page.goto('https://resource-owner.test/index.html', { waitUntil: 'load' })
      await installReload(page)
      const requested = page.waitForRequest((request) =>
        request.url().includes('/site.css?tola-representation')
      )
      const update = sendNextRevision(page, [modifiedOutput('site.css'), modifiedOutput('cover.png')])
      try {
        await requested
        await page.evaluate((mutation) => {
          const stylesheet = document.querySelector('link[rel=stylesheet]:not([data-tola-staging])')!
          if (mutation === 'stylesheet source') stylesheet.setAttribute('href', '/owned.css')
          else if (mutation === 'stylesheet removal') stylesheet.remove()
          else document.querySelector('#cover')!.setAttribute('src', '/owned.png')
        }, change)
      } finally {
        releaseStylesheet()
        await update
      }
      expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
      await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
      if (change === 'stylesheet removal') await expect(page.locator('link[rel=stylesheet]')).toHaveCount(0)
      else {await expect(page.locator('link[rel=stylesheet]')).toHaveAttribute(
          'href',
          change === 'stylesheet source' ? '/owned.css' : '/site.css',
        )}
      await expect(page.locator('#cover')).toHaveAttribute(
        'src',
        change === 'image source' ? '/owned.png' : '/cover.png',
      )
    })
  }

  test('stylesheet update awaits cold background', async ({ page }) => {
    const { promise: released, resolve: releaseImage } = Promise.withResolvers<void>()
    await page.route('https://cold-background.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/styles/site.css') {
        return route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: url.searchParams.has('tola-representation')
            ? '@property --brand { syntax: "<color>"; inherits: false; initial-value: blue; } @layer utilities { @supports (display: grid) { body { background: blue url("../new.svg"); } } }'
            : 'body { background: red; }',
        })
      }
      if (url.pathname === '/new.svg') {
        await released
        return route.fulfill({
          status: 200,
          contentType: 'image/svg+xml',
          body:
            '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1" fill="green"/></svg>',
        })
      }
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body:
          '<!doctype html><html><head><link rel="stylesheet" href="/styles/site.css"></head><body><input id="search" value="seed"></body></html>',
      })
    })
    await page.goto('https://cold-background.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    await page.locator('#search').fill('retained input')
    const requested = page.waitForRequest((request) => new URL(request.url()).pathname === '/new.svg')
    const update = sendNextRevision(page, [modifiedOutput('styles/site.css')])
    try {
      await requested
      expect(await runtimeState(page)).toEqual({ reloads: 0, revision: '0'.repeat(64) })
      await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
      await expect(page.locator('body')).toHaveCSS('background-image', 'none')
      await page.evaluate(async () => {
        for (let frame = 0; frame < 3; frame += 1) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          if (getComputedStyle(document.body).backgroundColor !== 'rgb(255, 0, 0)') {
            throw new Error('The background changed before its replacement image was ready')
          }
        }
      })
    } finally {
      releaseImage()
      await update
    }
    await expect(page.locator('body')).toHaveCSS('background-color', 'rgb(0, 0, 255)')
    await expect(page.locator('body')).toHaveCSS(
      'background-image',
      'url("https://cold-background.test/new.svg")',
    )
    await expect(page.locator('#search')).toHaveValue('retained input')
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
  })

  test('image preparation preserves request attributes', async ({ page }) => {
    const requestedImages: (string | undefined)[] = []
    const { promise: released, resolve: releaseImage } = Promise.withResolvers<void>()
    await page.route('https://image-request.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/cover.png') {
        if (url.searchParams.has('tola-representation')) {
          const headers = await route.request().allHeaders()
          requestedImages.push(headers.referer)
          if (requestedImages.length > 1) await released
        }
        return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
      }
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body:
          '<!doctype html><html><body><img id="cover" src="/cover.png" crossorigin="anonymous" referrerpolicy="no-referrer"></body></html>',
      })
    })
    await page.goto('https://image-request.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    try {
      await sendNextRevision(page, [modifiedOutput('cover.png')])
      const frames = await page.evaluate(async () => {
        const complete = []
        for (let frame = 0; frame < 3; frame += 1) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
          const image = document.querySelector('#cover') as HTMLImageElement
          complete.push(image.complete && image.naturalWidth > 0)
        }
        return complete
      })
      expect(requestedImages).toEqual([undefined])
      expect(frames).toEqual([true, true, true])
      expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
    } finally {
      releaseImage()
    }
  })

  for (const outcome of ['ready', 'failed'] as const) {
    test(`${outcome} image decoding gates resource updates`, async ({ page }) => {
      await page.route('https://decoded-resources.test/**', (route) => {
        const url = new URL(route.request().url())
        if (url.pathname === '/site.css') {
          return route.fulfill({
            status: 200,
            contentType: 'text/css',
            body: `body { color: ${url.searchParams.has('tola-representation') ? 'blue' : 'red'}; }`,
          })
        }
        if (url.pathname === '/cover.png') {
          return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
        }
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><img id="cover" src="/cover.png"><input id="search" value="seed"></body></html>',
        })
      })
      await page.goto('https://decoded-resources.test/index.html', { waitUntil: 'load' })
      await installReload(page)
      await page.locator('#search').fill('retained input')
      await page.evaluate(() => {
        const decoded = Promise.withResolvers<void>()
        const started = Promise.withResolvers<void>()
        const decode = HTMLImageElement.prototype.decode
        window.imageDecodeStarted = started.promise
        window.finishImageDecode = (failed) => {
          if (failed) decoded.reject(new DOMException('Cannot decode image', 'EncodingError'))
          else decoded.resolve()
        }
        HTMLImageElement.prototype.decode = function () {
          if (!this.src.includes('tola-representation')) return decode.call(this)
          started.resolve()
          return decoded.promise.then(() => decode.call(this))
        }
      })
      const update = sendNextRevision(page, [modifiedOutput('site.css'), modifiedOutput('cover.png')])
      try {
        await Promise.race([
          page.evaluate(() => window.imageDecodeStarted),
          update.then(() => {
            throw new Error('Resources committed before image decoding started')
          }),
        ])
        const frames = await page.evaluate(async () => {
          const appearances = []
          for (let frame = 0; frame < 3; frame += 1) {
            await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
            appearances.push({
              color: getComputedStyle(document.body).color,
              source: document.querySelector('#cover')?.getAttribute('src'),
              revision: (window as TolaWindow).Tola.revision,
            })
          }
          return appearances
        })
        expect(frames).toEqual(Array.from({ length: 3 }, () => ({
          color: 'rgb(255, 0, 0)',
          source: '/cover.png',
          revision: '0'.repeat(64),
        })))
        await page.evaluate((failed) => window.finishImageDecode?.(failed), outcome === 'failed')
        await update

        await expect(page.locator('#search')).toHaveValue('retained input')
        await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
        await expect(page.locator('body')).toHaveCSS(
          'color',
          outcome === 'ready' ? 'rgb(0, 0, 255)' : 'rgb(255, 0, 0)',
        )
        await expect(page.locator('#cover')).toHaveAttribute(
          'src',
          outcome === 'ready' ? /tola-representation=b{64}/ : '/cover.png',
        )
        expect(await runtimeState(page)).toEqual({
          reloads: outcome === 'ready' ? 0 : 1,
          revision: (outcome === 'ready' ? 'b' : '0').repeat(64),
        })
      } finally {
        await page.evaluate(() => window.finishImageDecode?.(false))
        await update
      }
    })
  }

  test('retained dependency capacity bounds updates', async ({ page }) => {
    const stylesheetResponses = [Promise.withResolvers<void>(), Promise.withResolvers<void>()] as const
    await page.route('https://retained-capacity.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/site.css') {
        const representation = url.searchParams.get('tola-representation')
        if (representation) await stylesheetResponses[representation.startsWith('b') ? 0 : 1].promise
        return route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: `body { color: ${
            representation === null ? 'red' : representation.startsWith('b') ? 'blue' : 'green'
          }; }`,
        })
      }
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body:
          '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body>Document</body></html>',
      })
    })
    await page.goto('https://retained-capacity.test/index.html', { waitUntil: 'load' })
    await page.evaluate(() => {
      const ResourceObserver = PerformanceObserver
      globalThis.PerformanceObserver = class extends ResourceObserver {
        constructor(callback: PerformanceObserverCallback) {
          super(callback)
          window.reportResourceUrls = (urls) =>
            callback({
              getEntries: () =>
                urls.map((
                  name,
                ) => ({
                  name,
                  initiatorType: new URL(name).pathname === '/site.css' ? 'link' : 'fetch',
                } as PerformanceResourceTiming)),
              getEntriesByName: () => [],
              getEntriesByType: () => [],
            }, this)
        }
      }
    })
    await installReload(page)
    try {
      for (const index of [0, 1] as const) {
        const requested = page.waitForRequest((request) =>
          request.url().includes(`tola-representation=${index === 0 ? 'b' : 'c'}`)
        )
        const update = sendNextRevision(page, [
          modifiedOutput('site.css', 'asset', (index === 0 ? 'b' : 'c').repeat(64)),
        ])
        try {
          await requested
          await page.evaluate((round) => {
            if (round === 0) {
              window.reportResourceUrls?.(
                Array.from({ length: 16383 }, (_, number) => `${location.origin}/retained-${number}.json`),
              )
              window.reportResourceUrls?.([
                `${location.origin}/site.css?tola-representation=${'f'.repeat(64)}`,
                `${location.origin}/retained-0.json?different-query=true`,
                'https://external.test/outside.json',
              ])
            } else {
              window.reportResourceUrls?.([`${location.origin}/beyond-capacity.json`])
            }
          }, index)
        } finally {
          stylesheetResponses[index].resolve()
          await update
        }
        await expect(page.locator('body')).toHaveCSS('color', 'rgb(0, 0, 255)')
        expect(await runtimeState(page)).toEqual({ reloads: index, revision: 'b'.repeat(64) })
        await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
      }
    } finally {
      for (const response of stylesheetResponses) response.resolve()
    }
  })

  test('timing buffer overflow retains dependencies', async ({ page }) => {
    await page.route('https://timing-capacity.test/**', (route) => {
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
          '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><input id="search" value="seed"></body></html>',
      })
    })
    await page.goto('https://timing-capacity.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    await page.locator('#search').fill('retained input')
    await page.evaluate(async () => {
      const full = new Promise<void>((resolve) => {
        performance.addEventListener('resourcetimingbufferfull', () => resolve(), { once: true })
      })
      performance.setResourceTimingBufferSize(1)
      await Promise.all(Array.from({ length: 3 }, (_, index) => fetch(`/tracked.txt?request=${index}`)))
      await full
    })

    await sendNextRevision(page, [modifiedOutput('site.css')])

    await expect(page.locator('body')).toHaveCSS('color', 'rgb(0, 0, 255)')
    await expect(page.locator('#search')).toHaveValue('retained input')
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })

    await page.evaluate(async () => {
      const observed = new Promise<void>((resolve) => {
        const observer = new PerformanceObserver((entries) => {
          if (entries.getEntries().some((entry) => entry.name.endsWith('/after-capacity.txt'))) {
            observer.disconnect()
            resolve()
          }
        })
        observer.observe({ type: 'resource' })
      })
      await fetch('/after-capacity.txt')
      await observed
    })
    await sendNextRevision(page, [modifiedOutput('after-capacity.txt')])

    expect(await runtimeState(page)).toEqual({ reloads: 1, revision: 'b'.repeat(64) })
    expect(await page.evaluate(() => (window as TolaWindow).Tola.revisionReloadRequired)).toBe(true)
  })

  test('site resources update without reload', async ({ page }) => {
    await page.route('https://assets.test/**', async (route) => {
      const url = route.request().url()
      if (url.endsWith('/index.html')) {
        await route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="https://assets.test/site.css"></head><body><img id="cover" src="https://assets.test/cover.png"></body></html>',
        })
      } else if (url.includes('.png')) {
        await route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
      } else {
        await route.fulfill({ status: 200, contentType: 'text/css', body: 'body {}' })
      }
    })
    await page.goto('https://assets.test/index.html', { waitUntil: 'load' })
    await installReload(page)

    await sendNextRevision(page, [modifiedOutput('site.css')])
    await expect(page.locator('link[rel~=stylesheet]')).toHaveAttribute('href', /tola-representation=b{64}/)
    await sendNextRevision(page, [modifiedOutput('cover.png')])
    await expect(page.locator('#cover')).toHaveAttribute('src', /tola-representation=[bc]{64}/)
    await expectReloads(page, 0)
  })

  test('css image revisions retain stylesheet address', async ({ page }) => {
    const requestedImages: string[] = []
    await page.route('https://css-dependency.test/**', async (route) => {
      const url = route.request().url()
      if (url.endsWith('/index.html')) {
        await route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><main>stable</main></body></html>',
        })
      } else if (new URL(url).pathname === '/site.css') {
        await route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: 'main { background-image: url("/background.png"); }',
        })
      } else {
        requestedImages.push(url)
        await route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
      }
    })
    await page.goto('https://css-dependency.test/index.html', { waitUntil: 'load' })
    await installReload(page)

    await sendNextRevision(page, [modifiedOutput('background.png')])

    await expectReloads(page, 0)
    await expect(await currentRevision(page)).toBe('b'.repeat(64))
    await expect(page.locator('link[rel~=stylesheet]')).toHaveAttribute(
      'href',
      'https://css-dependency.test/site.css',
    )
    await expect(page.locator('main')).toHaveCSS(
      'background-image',
      `url("https://css-dependency.test/background.png?tola-representation=${'b'.repeat(64)}")`,
    )
    expect(requestedImages.some((url) => url.includes(`tola-representation=${'b'.repeat(64)}`))).toBe(true)

    await sendNextRevision(page, [modifiedOutput('site.css', 'asset', 'c'.repeat(64))])

    await expect(page.locator('main')).toHaveCSS(
      'background-image',
      `url("https://css-dependency.test/background.png?tola-representation=${'b'.repeat(64)}")`,
    )
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'c'.repeat(64) })
  })

  /** Each markup consumes the same modified `cover.png` through a different attribute. */
  const imageConsumers: readonly {
    name: string
    markup: string
    check?: (page: Page) => Promise<void>
  }[] = [
    {
      name: 'shared image',
      markup: '<img id="cover" src="/cover.png"><main style="background-image:url(/cover.png)">Cover</main>',
    },
    {
      name: 'srcset image',
      markup: '<img id="cover" src="/cover.png" srcset="/cover.png 1x, /cover.png 2x">',
    },
    {
      name: 'svg-consumed image',
      markup:
        '<img id="cover" src="/cover.png"><svg><image href="/cover.png" width="10" height="10" /></svg>',
      check: (page) => expect(page.locator('svg image')).toHaveAttribute('href', '/cover.png'),
    },
  ]

  for (const consumer of imageConsumers) {
    test(`${consumer.name} change reloads page`, async ({ page }) => {
      await serveImagePage(
        page,
        'https://image.test',
        `<!doctype html><html><body>${consumer.markup}</body></html>`,
      )

      await sendNextRevision(page, [modifiedOutput('cover.png')])

      expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
      await expect(page.locator('#cover')).toHaveAttribute('src', '/cover.png')
      await consumer.check?.(page)
    })
  }

  test('base URL resolves relative media', async ({ page }) => {
    await serveImagePage(
      page,
      'https://based-image.test',
      '<!doctype html><html><head><base href="/images/"></head><body><img id="cover" src="cover.png"></body></html>',
    )

    await sendNextRevision(page, [modifiedOutput('images/cover.png')])

    await expect(page.locator('#cover')).toHaveAttribute(
      'src',
      /^https:\/\/based-image\.test\/images\/cover\.png\?tola-representation=b{64}$/,
    )
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
  })

  test('pending CSS delays resource commit', async ({ page }) => {
    let releaseStylesheet!: () => void
    const released = new Promise<void>((resolve) => {
      releaseStylesheet = resolve
    })
    await page.route('https://concurrent-resources.test/**', async (route) => {
      const url = new URL(route.request().url())
      const pending = url.searchParams.has('tola-representation')
      if (url.pathname === '/site.css') {
        if (pending) await released
        await route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: `body { color: ${pending ? 'blue' : 'red'}; }`,
        })
      } else if (url.pathname === '/cover.png') {
        await route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
      } else {
        await route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><img id="cover" src="/cover.png"></body></html>',
        })
      }
    })
    await page.goto('https://concurrent-resources.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    const imageRequested = page.waitForRequest((request) => {
      const url = new URL(request.url())
      return url.pathname === '/cover.png' && url.searchParams.has('tola-representation')
    }, { timeout: 3000 })
    const update = sendNextRevision(page, [modifiedOutput('site.css'), modifiedOutput('cover.png')])
    try {
      await imageRequested
      await expect(page.locator('#cover')).toHaveAttribute('src', '/cover.png')
      await expect(page.locator('body')).toHaveCSS('color', 'rgb(255, 0, 0)')
      expect(await currentRevision(page)).toBe('0'.repeat(64))
      releaseStylesheet()
      await update
      await expect(page.locator('#cover')).toHaveAttribute('src', /tola-representation=b{64}/)
      await expect(page.locator('body')).toHaveCSS('color', 'rgb(0, 0, 255)')
      expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
    } finally {
      releaseStylesheet()
      await update
    }
  })

  test('failed image discards stylesheet update', async ({ page }) => {
    let releaseImage!: () => void
    const released = new Promise<void>((resolve) => {
      releaseImage = resolve
    })
    await page.route('https://timed-out-image.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/index.html') {
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><img id="cover" src="/cover.png" loading="eager"></body></html>',
        })
      }
      if (url.pathname === '/site.css') {
        return route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: url.searchParams.has('tola-representation')
            ? 'body { color: blue; }'
            : 'body { color: red; }',
        })
      }
      if (url.searchParams.has('tola-representation')) await released
      return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
    })
    await page.goto('https://timed-out-image.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    await page.clock.install()
    const requested = page.waitForRequest((request) => {
      const url = new URL(request.url())
      return url.pathname === '/cover.png' && url.searchParams.has('tola-representation')
    })
    const update = sendNextRevision(page, [modifiedOutput('site.css'), modifiedOutput('cover.png')])
    try {
      await requested
      await expect(page.locator('link[data-tola-staging]')).toHaveCount(1)
      await expect(page.locator('body')).toHaveCSS('color', 'rgb(255, 0, 0)')
      await page.clock.fastForward(5000)
      await update

      await expect(page.locator('link[data-tola-staging]')).toHaveCount(0)
      await expect(page.locator('link[rel~=stylesheet]')).toHaveAttribute('href', '/site.css')
      await expect(page.locator('body')).toHaveCSS('color', 'rgb(255, 0, 0)')
      await expect(page.locator('#cover')).toHaveAttribute('src', '/cover.png')
      await expect(page.locator('#cover')).toHaveAttribute('loading', 'eager')
      expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
    } finally {
      releaseImage()
      await update
    }
  })

  for (const resource of ['site.css', 'cover.png']) {
    test(`${resource} edits retain scripted pages`, async ({ page }) => {
      await page.route('https://scripted-resources.test/**', (route) => {
        const url = new URL(route.request().url())
        if (url.pathname === '/site.css') {
          return route.fulfill({
            status: 200,
            contentType: 'text/css',
            body: `body { color: ${url.searchParams.has('tola-representation') ? 'blue' : 'red'}; }`,
          })
        }
        if (url.pathname === '/cover.png') {
          return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
        }
        return route.fulfill({
          status: 200,
          contentType: 'text/html',
          body:
            '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><img id="cover" src="/cover.png"><input id="search" value="seed"><script>window.resourceApplication = {};</script></body></html>',
        })
      })
      await page.goto('https://scripted-resources.test/index.html', { waitUntil: 'load' })
      await installReload(page)
      const application = await page.evaluateHandle(() => window.resourceApplication)
      await page.locator('#search').fill('script-owned input')

      await sendNextRevision(page, [modifiedOutput(resource)])

      expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
      expect(await application.evaluate((owned) => owned === window.resourceApplication)).toBe(true)
      await expect(page.locator('#search')).toHaveValue('script-owned input')
      if (resource === 'site.css') await expect(page.locator('body')).toHaveCSS('color', 'rgb(0, 0, 255)')
      else await expect(page.locator('#cover')).toHaveAttribute('src', /tola-representation=b{64}/)
      await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
    })
  }

  test('late script retains resource commit', async ({ page }) => {
    const { promise: released, resolve: release } = Promise.withResolvers<void>()
    await page.route('https://late-script.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/site.css') {
        const pending = url.searchParams.has('tola-representation')
        if (pending) await released
        return route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: `body { color: ${pending ? 'blue' : 'red'}; }`,
        })
      }
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body:
          '<!doctype html><html><head><link rel="stylesheet" href="/site.css"></head><body><input id="search" value="seed"></body></html>',
      })
    })
    await page.goto('https://late-script.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    const pending = page.waitForRequest((request) => request.url().includes('tola-representation'))
    const update = sendNextRevision(page, [modifiedOutput('site.css')])
    try {
      await pending
      await page.evaluate(() => {
        const script = document.createElement('script')
        script.textContent = "document.getElementById('search').value = 'script-owned';"
        document.head.appendChild(script)
        script.remove()
      })
    } finally {
      release()
      await update
    }

    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
    await expect(page.locator('body')).toHaveCSS('color', 'rgb(0, 0, 255)')
    await expect(page.locator('#search')).toHaveValue('script-owned')
    await expect(page.locator('[data-tola-staging]')).toHaveCount(0)
  })
})
