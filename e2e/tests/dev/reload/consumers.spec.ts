import { expect, type Page, test } from '@playwright/test'
import { installReload, modifiedOutput, runtimeState, sendNextRevision } from '../../../support/reload.ts'

declare global {
  interface Window {
    indexReady?: boolean
    searchTitle?: string
    searchClicks?: number
    consumerStarted?: boolean
    consumerAborted?: boolean
    lateRejected?: boolean
    updateFrozen?: boolean
    scriptReady?: boolean
    scriptRuns?: number
  }
}

type ConsumerMode = 'accept' | 'none' | 'reject' | 'timeout' | 'late' | 'conflict' | 'script' | 'document'

async function consumerPage(page: Page, mode: ConsumerMode, holdIndex = false) {
  let updated = false
  let updatedClasses = false
  const { promise: indexResponse, resolve: releaseIndex } = Promise.withResolvers<void>()
  const script = `
const mode = ${JSON.stringify(mode)};
let title = '';
let clicks = 0;
async function refreshIndex(signal) {
  const response = await fetch('/search-index.json', { cache: 'no-store', signal });
  if (!response.ok) throw new Error('Index could not be loaded');
  title = (await response.json()).title;
  window.searchTitle = title;
}
refreshIndex().then(() => { window.indexReady = true; });
document.getElementById('search').addEventListener('click', () => {
  window.searchClicks = ++clicks;
  document.getElementById('result').textContent = title;
});
if (mode === 'script') {
  const script = document.createElement('script');
  script.src = '/worker.js';
  script.onload = () => { script.remove(); window.scriptReady = true; };
  document.head.appendChild(script);
}
async function consume({ signal }) {
  window.consumerStarted = true;
  signal.addEventListener('abort', () => { window.consumerAborted = true; }, { once: true });
  if (mode === 'reject') throw new Error('Index update failed');
  if (mode === 'timeout') return new Promise(resolve => signal.addEventListener('abort', resolve, { once: true }));
  await refreshIndex(signal);
}
if (mode !== 'none') window.addEventListener('tola:before-update', event => {
  const detail = event.detail;
  window.updateFrozen = Object.isFrozen(detail) && Object.isFrozen(detail.changes) && detail.changes.every(Object.isFrozen);
  const path = mode === 'script' ? 'worker.js' : mode === 'document' ? 'index.html' : 'search-index.json';
  if (mode === 'late') {
    Promise.resolve().then(() => {
      try { detail.accept([path], consume); }
      catch (_) { window.lateRejected = true; }
    });
    return;
  }
  detail.accept([path], consume);
  if (mode === 'conflict') detail.accept([path], consume);
});`
  await page.route('https://consumer-updates.test/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (url.pathname === '/search-index.json') {
      if (updated && holdIndex) await indexResponse
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ title: updated ? 'Second' : 'First' }),
      })
    }
    if (url.pathname === '/worker.js') {
      return route.fulfill({
        status: 200,
        contentType: 'text/javascript',
        body: `window.scriptRuns = (window.scriptRuns || 0) + ${updated ? 2 : 1};`,
      })
    }
    if (url.pathname === '/theme.css') {
      return route.fulfill({
        status: 200,
        contentType: 'text/css',
        body: url.searchParams.has('tola-representation')
          ? '.blue { background-color: rgb(0, 0, 255); }'
          : '.red { background-color: rgb(255, 0, 0); }',
      })
    }
    const sourceUpdated = updatedClasses && request.headers()['x-tola-revision'] !== '0'.repeat(64)
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        `<!doctype html><html><head><link rel="stylesheet" href="/theme.css"></head><body><section id="panel" class="${
          sourceUpdated ? 'blue' : 'red'
        }"><input id="query" value="seed"><button id="search">Search</button><p id="result"></p></section><script>${script}</script></body></html>`,
    })
  })
  await page.goto('https://consumer-updates.test/index.html', { waitUntil: 'load' })
  await page.waitForFunction(() => window.indexReady === true)
  if (mode === 'script') await page.waitForFunction(() => window.scriptReady === true)
  await installReload(page)
  return {
    publish: (visual = false) => {
      updated = true
      updatedClasses = visual
    },
    releaseIndex,
  }
}

test('accepted index refresh precedes visual commit', async ({ page }) => {
  const site = await consumerPage(page, 'accept', true)
  await page.locator('#search').click()
  await expect(page.locator('#result')).toHaveText('First')
  await page.locator('#query').fill('retained query')
  const panel = await page.locator('#panel').elementHandle()
  site.publish(true)
  const requested = page.waitForRequest((request) => new URL(request.url()).pathname === '/search-index.json')
  const update = sendNextRevision(page, [
    modifiedOutput('search-index.json'),
    modifiedOutput('theme.css'),
    modifiedOutput('index.html', 'html-document'),
  ])
  try {
    await Promise.race([
      requested,
      update.then(() => {
        throw new Error('Visual update completed before its index was refreshed')
      }),
    ])
    expect(await page.evaluate(() => window.consumerStarted)).toBe(true)
    await expect(page.locator('#panel')).toHaveClass('red')
    await expect(page.locator('#panel')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: '0'.repeat(64) })
  } finally {
    site.releaseIndex()
    await update
  }

  await expect(page.locator('#panel')).toHaveClass('blue')
  await expect(page.locator('#panel')).toHaveCSS('background-color', 'rgb(0, 0, 255)')
  expect(await panel!.evaluate((element) => element === document.querySelector('#panel'))).toBe(true)
  await expect(page.locator('#query')).toHaveValue('retained query')
  expect(await page.evaluate(() => window.updateFrozen)).toBe(true)
  await page.locator('#search').click()
  await expect(page.locator('#result')).toHaveText('Second')
  expect(await page.evaluate(() => window.searchClicks)).toBe(2)
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
})

for (const mode of ['none', 'reject', 'late', 'conflict'] as const) {
  test(`${mode} index consumer requires navigation`, async ({ page }) => {
    const site = await consumerPage(page, mode)
    site.publish()

    await sendNextRevision(page, [modifiedOutput('search-index.json')])

    if (mode === 'late') expect(await page.evaluate(() => window.lateRejected)).toBe(true)
    if (mode === 'reject') expect(await page.evaluate(() => window.consumerAborted)).toBe(true)
    expect(await page.evaluate(() => window.searchTitle)).toBe('First')
    expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
  })
}

test('index deadline aborts pending consumer', async ({ page }) => {
  const site = await consumerPage(page, 'timeout')
  await page.clock.install()
  site.publish()
  const update = sendNextRevision(page, [modifiedOutput('search-index.json')])
  try {
    await expect.poll(() => page.evaluate(() => window.consumerStarted)).toBe(true)
    await page.clock.fastForward(5001)
    await update
  } finally {
    await page.clock.fastForward(5001)
  }

  expect(await page.evaluate(() => window.consumerAborted)).toBe(true)
  expect(await page.evaluate(() => window.searchTitle)).toBe('First')
  expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
})

test('page exit aborts pending index refresh', async ({ page }) => {
  const site = await consumerPage(page, 'accept', true)
  site.publish()
  const update = sendNextRevision(page, [modifiedOutput('search-index.json')])
  try {
    await expect.poll(() => page.evaluate(() => window.consumerStarted)).toBe(true)
    await page.evaluate(() => dispatchEvent(new PageTransitionEvent('pagehide')))
    await update
  } finally {
    site.releaseIndex()
  }

  expect(await page.evaluate(() => window.consumerAborted)).toBe(true)
  expect(await page.evaluate(() => window.searchTitle)).toBe('First')
  expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
})

for (const mode of ['script', 'document'] as const) {
  test(`${mode} consumer cannot claim output`, async ({ page }) => {
    const site = await consumerPage(page, mode)
    site.publish(mode === 'document')

    await sendNextRevision(page, [
      modifiedOutput(
        mode === 'script' ? 'worker.js' : 'index.html',
        mode === 'script' ? 'asset' : 'html-document',
      ),
    ])

    expect(await page.evaluate(() => window.consumerStarted)).not.toBe(true)
    if (mode === 'script') expect(await page.evaluate(() => window.scriptRuns)).toBe(1)
    expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
  })
}
