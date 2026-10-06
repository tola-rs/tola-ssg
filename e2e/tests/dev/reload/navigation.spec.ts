import { expect } from '@playwright/test'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import {
  expectReloads,
  installReload,
  modifiedOutput,
  reloadDocument,
  runtimeState,
  sendNextRevision,
  serveDocument,
  type TolaRuntime,
  type TolaWindow,
  waitForOpenSocket,
} from '../../../support/reload.ts'
import { test } from '../../../support/server.ts'

declare global {
  interface Window {
    clicked?: string[]
    appStore?: string
    submitted?: string
    initialRuntime?: TolaRuntime
    initialSocket?: WebSocket | null
    fragmentLoaded?: boolean
    fragmentExecutions?: number
    mediaInterruptions?: Record<string, string[]>
  }
}

function silentWav(seconds = 2): Buffer {
  const sampleRate = 8000
  const samples = sampleRate * seconds
  const bodySize = samples * 2
  const wav = Buffer.alloc(44 + bodySize)
  wav.write('RIFF', 0)
  wav.writeUInt32LE(36 + bodySize, 4)
  wav.write('WAVEfmt ', 8)
  wav.writeUInt32LE(16, 16)
  wav.writeUInt16LE(1, 20)
  wav.writeUInt16LE(1, 22)
  wav.writeUInt32LE(sampleRate, 24)
  wav.writeUInt32LE(sampleRate * 2, 28)
  wav.writeUInt16LE(2, 32)
  wav.writeUInt16LE(16, 34)
  wav.write('data', 36)
  wav.writeUInt32LE(bodySize, 40)
  return wav
}

function wavUrl(seconds = 2): string {
  return `data:audio/wav;base64,${silentWav(seconds).toString('base64')}`
}

function htmlProgram(body: string, script: string, path = 'index.html') {
  return `#document(${
    JSON.stringify(path)
  }, format: "html")[#html.html[#html.head[]#html.body[${body}#html.script(${JSON.stringify(script)})]]]\n`
}

test('reload restores scroll position', async ({ page }) => {
  await page.addInitScript(() => {
    history.scrollRestoration = 'manual'
  })
  let version = 1
  await page.route('https://reload.test/**', async (route) => {
    const body = reloadDocument({
      body: `<main style="height:2400px"><p id="version">version ${version}</p></main>`,
    })
    await route.fulfill({ status: 200, contentType: 'text/html', body })
  })
  await page.goto('https://reload.test/index.html', { waitUntil: 'load' })
  await page.evaluate(() => window.scrollTo(0, 600))

  version = 2
  await page.evaluate(() => {
    ;(window as TolaWindow).Tola.saveReloadState()
  })
  await expect(await page.evaluate(() => sessionStorage.getItem('tola:reload-position'))).not.toBeNull()

  await page.reload({ waitUntil: 'load' })
  await page.waitForFunction(() => document.querySelector('#version')?.textContent === 'version 2')
  await page.waitForFunction(() => Boolean((window as TolaWindow).Tola.timings['reload-restored']))
  await page.waitForFunction(() => Math.abs(window.scrollY - 600) < 2)
  expect(await page.evaluate(() => sessionStorage.getItem('tola:reload-position'))).toBeNull()
})

test('text edits retain uninterrupted media', async ({ page }) => {
  let text = 'First'
  const source = wavUrl(8)
  await page.route('https://media-edit.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        `<!doctype html><html><body><p id="text">${text}</p><audio id="audio" src="${source}"></audio><video id="video" src="${source}"></video></body></html>`,
    }))
  await page.goto('https://media-edit.test/index.html', { waitUntil: 'load' })
  await installReload(page)
  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll('audio,video'))
      .every((element) => (element as HTMLMediaElement).readyState >= 1)
  )
  await page.evaluate(() => {
    for (const element of document.querySelectorAll<HTMLMediaElement>('audio,video')) {
      element.currentTime = 0.5
      element.volume = 0.4
      element.playbackRate = 1.25
    }
  })
  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll('audio,video'))
      .every((element) => !(element as HTMLMediaElement).seeking)
  )
  await page.evaluate(() => {
    window.mediaInterruptions = { audio: [], video: [] }
    for (const element of document.querySelectorAll('audio,video')) {
      for (const event of ['seeking', 'ratechange', 'play', 'pause']) {
        element.addEventListener(event, () => {
          window.mediaInterruptions?.[element.id]?.push(event)
        })
      }
    }
  })
  text = 'Second'

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('#text')).toHaveText('Second')
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  for (const tag of ['audio', 'video']) {
    expect(await page.evaluate((tag) => window.mediaInterruptions?.[tag], tag)).toEqual([])
    expect(
      await page.locator(`#${tag}`).evaluate((element) => ({
        time: (element as HTMLMediaElement).currentTime,
        paused: (element as HTMLMediaElement).paused,
        volume: (element as HTMLMediaElement).volume,
        rate: (element as HTMLMediaElement).playbackRate,
      })),
    ).toEqual({ time: 0.5, paused: true, volume: 0.4, rate: 1.25 })
  }
  await page.evaluate(async () => {
    const media = Array.from(document.querySelectorAll<HTMLMediaElement>('audio,video'))
    await Promise.all(media.map(async (element) => {
      element.muted = true
      await element.play()
    }))
    window.mediaInterruptions = { audio: [], video: [] }
  })
  text = 'Third'

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('#text')).toHaveText('Third')
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  expect(
    await page.evaluate(() => ({
      interruptions: window.mediaInterruptions,
      playing: Array.from(document.querySelectorAll<HTMLMediaElement>('audio,video'))
        .every((element) => !element.paused && element.currentTime >= 0.5),
    })),
  ).toEqual({ interruptions: { audio: [], video: [] }, playing: true })
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'c'.repeat(64) })
})

test('replaced content retains native state', async ({ page }) => {
  let wrapper = 'section'
  const source = wavUrl(8)
  await page.route('https://replacement-state.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        `<!doctype html><html><body><${wrapper} id="panel"><input id="title" value="seed"><audio id="preview" src="${source}"></audio></${wrapper}></body></html>`,
    }))
  await page.goto('https://replacement-state.test/index.html', { waitUntil: 'load' })
  await installReload(page)
  await page.waitForFunction(() => (document.querySelector('#preview') as HTMLMediaElement).readyState >= 1)
  const original = await page.locator('#panel').elementHandle()
  await page.locator('#title').fill('edited title')
  await page.evaluate(() => {
    const title = document.querySelector('#title') as HTMLInputElement
    title.setSelectionRange(2, 8, 'forward')
    const media = document.querySelector('#preview') as HTMLMediaElement
    media.currentTime = 0.5
    media.volume = 0.4
    media.playbackRate = 1.25
  })
  await page.waitForFunction(() => !(document.querySelector('#preview') as HTMLMediaElement).seeking)
  wrapper = 'div'

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('div#panel')).toHaveCount(1)
  expect(await original!.evaluate((element) => element.isConnected)).toBe(false)
  await expect(page.locator('#title')).toHaveValue('edited title')
  expect(
    await page.locator('#title').evaluate((element) => {
      const title = element as HTMLInputElement
      return {
        focused: document.activeElement === title,
        selection: [title.selectionStart, title.selectionEnd, title.selectionDirection],
      }
    }),
  ).toEqual({ focused: true, selection: [2, 8, 'forward'] })
  await page.waitForFunction(() => {
    const media = document.querySelector('#preview') as HTMLMediaElement
    return media.readyState >= 1 && !media.seeking && Math.abs(media.currentTime - 0.5) < 0.01
  })
  expect(
    await page.locator('#preview').evaluate((element) => ({
      paused: (element as HTMLMediaElement).paused,
      volume: (element as HTMLMediaElement).volume,
      rate: (element as HTMLMediaElement).playbackRate,
    })),
  ).toEqual({ paused: true, volume: 0.4, rate: 1.25 })
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
})

test('embedded document change reloads page', async ({ page }) => {
  await page.route('https://frame.test/**', (route) => {
    const url = route.request().url()
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: url.endsWith('/preview.html')
        ? '<!doctype html><html><body>preview</body></html>'
        : '<!doctype html><html><body><iframe src="/preview.html"></iframe></body></html>',
    })
  })
  await page.goto('https://frame.test/index.html', { waitUntil: 'load' })
  await installReload(page)

  await sendNextRevision(page, [modifiedOutput('preview.html', 'html-document')])

  await expectReloads(page, 1)
  await expect(await page.evaluate(() => (window as TolaWindow).Tola.revision)).toBe('0'.repeat(64))
})

test('reload restores saved page state', async ({ page }) => {
  await page.route('https://state.test/**', (route) => {
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: reloadDocument({
        body: `
        <form id="profile">
          <input id="title" name="title" value="seed">
          <input id="enabled" type="checkbox">
          <input id="secret" type="password" value="">
        </form>
        <audio id="preview" preload="auto" src="${wavUrl()}"></audio>
      `,
      }),
    })
  })
  await page.goto('https://state.test/index.html', { waitUntil: 'load' })
  await page.waitForFunction(() => (document.querySelector('#preview') as HTMLMediaElement).readyState >= 4)
  const savedTime = await page.evaluate(() => {
    const title = document.querySelector('#title') as HTMLInputElement
    title.value = 'edited title'
    title.focus()
    title.setSelectionRange(2, 8, 'forward')
    ;(document.querySelector('#enabled') as HTMLInputElement).checked = true
    ;(document.querySelector('#secret') as HTMLInputElement).value = 'must-not-persist'
    const media = document.querySelector('#preview') as HTMLMediaElement
    media.currentTime = 0.5
    media.pause()
    media.volume = 0.4
    media.playbackRate = 1.25
    ;(window as TolaWindow).Tola.saveReloadState()
    return media.currentTime
  })
  expect(savedTime).toBeGreaterThan(0.2)
  await expect(await page.evaluate(() => sessionStorage.getItem('tola:reload-position')))
    .not.toContain('must-not-persist')

  await page.reload({ waitUntil: 'load' })
  await page.waitForFunction(() => Boolean((window as TolaWindow).Tola.timings['reload-restored']))
  await page.waitForFunction(
    (expected) =>
      Math.abs((document.querySelector('#preview') as HTMLMediaElement).currentTime - expected) < 0.05,
    savedTime,
  )

  await expect(page.locator('#title')).toHaveValue('edited title')
  await expect(page.locator('#enabled')).toBeChecked()
  await expect(await page.evaluate(() => document.activeElement?.id)).toBe('title')
  await expect(
    await page.evaluate(() => {
      const title = document.querySelector('#title') as HTMLInputElement
      return [title.selectionStart, title.selectionEnd, title.selectionDirection]
    }),
  ).toEqual([2, 8, 'forward'])
  const mediaState = await page.evaluate(() => {
    const media = document.querySelector('#preview') as HTMLMediaElement
    return {
      currentTime: media.currentTime,
      paused: media.paused,
      volume: media.volume,
      playbackRate: media.playbackRate,
    }
  })
  expect(mediaState.currentTime).toBeCloseTo(savedTime, 1)
  expect(mediaState).toMatchObject({ paused: true, volume: 0.4, playbackRate: 1.25 })
})

test('new site behavior owns reload state', async ({ page }) => {
  let scripted = false
  await page.addInitScript(() => {
    history.scrollRestoration = 'manual'
  })
  const body =
    '<main style="height:2400px"><input id="title" value="seed"><input id="enabled" type="checkbox"><select id="choice"><option>First</option><option>Second</option></select><button id="owner">Owner</button></main>'
  const script =
    "document.getElementById('title').value = 'seed'; document.getElementById('enabled').checked = false; document.getElementById('choice').selectedIndex = 0; document.getElementById('owner').focus(); window.scrollTo(0,900);"
  await page.route('https://activation-state.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: reloadDocument({ body: body + (scripted ? `<script>${script}</script>` : '') }),
    }))
  await page.goto('https://activation-state.test/index.html', { waitUntil: 'load' })
  await page.locator('#title').fill('saved native value')
  await page.evaluate(() => {
    ;(document.getElementById('enabled') as HTMLInputElement).checked = true
    ;(document.getElementById('choice') as HTMLSelectElement).selectedIndex = 1
    ;(document.getElementById('title') as HTMLInputElement).focus()
    window.scrollTo(0, 600)
    ;(window as TolaWindow).Tola.saveReloadState()
  })
  scripted = true
  await page.reload({ waitUntil: 'load' })
  await page.evaluate(async () => {
    const { promise, resolve } = Promise.withResolvers<void>()
    requestAnimationFrame(() => resolve())
    await promise
  })

  await expect(page.locator('#title')).toHaveValue('seed')
  await expect(page.locator('#enabled')).not.toBeChecked()
  expect(await page.locator('#choice').inputValue()).toBe('First')
  expect(await page.evaluate(() => ({ focus: document.activeElement?.id, scroll: window.scrollY })))
    .toEqual({ focus: 'owner', scroll: 900 })
  expect(await page.evaluate(() => sessionStorage.getItem('tola:reload-position'))).toBeNull()
})

test('late site behavior owns media initialization', async ({ page }) => {
  let restored = false
  await page.route('https://activation-media.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: reloadDocument({
        body: `<audio id="preview" preload="${restored ? 'none' : 'auto'}" src="${wavUrl()}"></audio>`,
      }),
    }))
  await page.goto('https://activation-media.test/index.html', { waitUntil: 'load' })
  await page.waitForFunction(() => (document.querySelector('#preview') as HTMLMediaElement).readyState >= 1)
  await page.evaluate(() => {
    const media = document.querySelector('#preview') as HTMLMediaElement
    media.currentTime = 0.5
    media.volume = 0.4
    media.playbackRate = 1.25
    ;(window as TolaWindow).Tola.saveReloadState()
  })
  restored = true
  await page.reload({ waitUntil: 'load' })
  await page.waitForFunction(() => (window as TolaWindow).Tola.timings['reload-restored'] !== undefined)
  expect(await page.locator('#preview').evaluate((media) => (media as HTMLMediaElement).readyState)).toBe(0)
  await page.evaluate(async () => {
    const media = document.querySelector('#preview') as HTMLMediaElement
    const { promise, resolve } = Promise.withResolvers<void>()
    media.addEventListener('loadedmetadata', () => resolve(), { once: true })
    const script = document.createElement('script')
    script.textContent =
      "const media = document.getElementById('preview'); media.load(); media.volume = 0.8; media.playbackRate = 0.75; media.muted = true; media.pause();"
    document.head.appendChild(script)
    script.remove()
    await promise
  })

  expect(
    await page.locator('#preview').evaluate((media) => ({
      volume: (media as HTMLMediaElement).volume,
      rate: (media as HTMLMediaElement).playbackRate,
      time: (media as HTMLMediaElement).currentTime,
      paused: (media as HTMLMediaElement).paused,
      muted: (media as HTMLMediaElement).muted,
    })),
  ).toEqual({ volume: 0.8, rate: 0.75, time: 0, paused: true, muted: true })
})

test('changed identity skips saved state', async ({ page }) => {
  let changed = false
  await page.route('https://structure.test/**', (route) => {
    const formId = changed ? 'replacement-profile' : 'profile'
    const media = changed ? wavUrl(1) : wavUrl(2)
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: reloadDocument({
        body: `
        <form id="${formId}"><input id="title" value="seed"></form>
        <audio id="preview" src="${media}"></audio>
      `,
      }),
    })
  })
  await page.goto('https://structure.test/index.html', { waitUntil: 'load' })
  await page.waitForFunction(() => (document.querySelector('#preview') as HTMLMediaElement).readyState >= 1)
  await page.evaluate(() => {
    const title = document.querySelector('#title') as HTMLInputElement
    title.value = 'should be skipped'
    title.focus()
    const media = document.querySelector('#preview') as HTMLMediaElement
    media.currentTime = 0.5
    media.volume = 0.25
    ;(window as TolaWindow).Tola.saveReloadState()
  })

  changed = true
  await page.reload({ waitUntil: 'load' })
  await page.waitForFunction(() => Boolean((window as TolaWindow).Tola.timings['reload-restored']))

  await expect(page.locator('#title')).toHaveValue('seed')
  await expect(await page.evaluate(() => document.activeElement?.id)).not.toBe('title')
  await expect(await page.evaluate(() => (document.querySelector('#preview') as HTMLMediaElement).volume))
    .toBe(1)
})

test('added output takes over fallback', async ({ page }) => {
  await serveDocument(page, {
    body:
      '<!doctype html><html><head><base href="/images/"></head><body><main>Missing route</main></body></html>',
    url: 'https://fallback.test/docs/missing/',
    status: 404,
    output: '404.html',
    pathPrefix: 'docs',
  })

  for (const path of ['another/index.html', 'missing/index.html']) {
    await page.evaluate(async (path) => {
      const tola = (window as TolaWindow).Tola
      tola.handleMessage({
        type: 'revision',
        page_availability: 'present',
        diff: {
          from: tola.revision,
          to: path === 'another/index.html' ? 'b'.repeat(64) : 'c'.repeat(64),
          changes: [{
            operation: 'added',
            output: { path, kind: 'html-document', representation: 'a'.repeat(64), size: 1 },
          }],
        },
      })
      await tola.revisionUpdates
    }, path)
    expect(await runtimeState(page))
      .toEqual({ reloads: path === 'another/index.html' ? 0 : 1, revision: 'b'.repeat(64) })
    await expect(page.locator('main')).toHaveText('Missing route')
  }
})

test('published route replaces 404 tab', async ({ page, sites }) => {
  let bundle = '#document("index.html")[Home]\n#document("404.html")[Missing route]\n'
  const site = await sites.dev({
    initialContent: null,
    beforeStart: (root) => writeFile(join(root, 'site.typ'), bundle),
  })
  const response = await page.goto(new URL('missing/', site.url).href, { waitUntil: 'load' })
  expect(response?.status()).toBe(404)
  await expect(page.locator('body')).toContainText('Missing route')
  await waitForOpenSocket(page)
  const before = await page.evaluate(() => {
    const runtime = window as TolaWindow
    return { revision: runtime.Tola.revision, navigation: performance.timeOrigin }
  })

  bundle += '#document("another/index.html")[Another page]\n'
  await writeFile(join(site.root, 'site.typ'), bundle)
  await page.waitForFunction((revision) => {
    const runtime = window as TolaWindow
    return runtime.Tola.revision !== revision
  }, before.revision)
  await expect(page.locator('body')).toContainText('Missing route')
  expect(await page.evaluate(() => performance.timeOrigin)).toBe(before.navigation)

  const navigation = page.waitForEvent('load')
  bundle += '#document("missing/index.html")[Now present]\n'
  await writeFile(join(site.root, 'site.typ'), bundle)
  await navigation

  await expect(page.locator('body')).toContainText('Now present')
  expect(await page.evaluate(() => performance.timeOrigin)).not.toBe(before.navigation)
  expect((await page.request.get(page.url())).status()).toBe(200)
})

test('reordered buttons retain matching actions', async ({ page, sites }) => {
  const script =
    'window.clicked = []; document.querySelectorAll("button").forEach(button => { const initialId = button.id; button.addEventListener("click", () => window.clicked.push(initialId)); });'
  const buttonPage = (order: string[]) => {
    const buttons = order.map((id) => `#html.elem("button", attrs: (id: "${id}"))[${id}]`).join('\n        ')
    return htmlProgram(`#html.elem("main")[${buttons}]`, script)
  }
  const site = await sites.dev({
    initialContent: null,
    beforeStart: (root) => writeFile(join(root, 'site.typ'), buttonPage(['alpha', 'beta'])),
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await waitForOpenSocket(page)
  const before = await page.evaluate(() => ({
    revision: (window as TolaWindow).Tola.revision,
    timeOrigin: performance.timeOrigin,
  }))

  await writeFile(join(site.root, 'site.typ'), buttonPage(['beta', 'alpha']))
  await page.waitForFunction(({ revision }) => {
    const runtime = (window as TolaWindow).Tola
    return typeof runtime?.revision === 'string' &&
      /^[0-9a-f]{64}$/.test(runtime.revision) &&
      runtime.revision !== revision &&
      document.querySelector('button')?.id === 'beta' &&
      runtime.timings.initialized !== undefined
  }, before)
  await page.locator('#alpha').click()

  expect(await page.evaluate(() => window.clicked)).toEqual(['alpha'])
  expect(await page.evaluate(() => performance.timeOrigin)).not.toBe(before.timeOrigin)
})

test('scripted content reinitializes after edits', async ({ page, sites }) => {
  const script =
    "document.getElementById('status').textContent = 'script-ready'; document.getElementById('panel').setAttribute('data-origin', 'script-owned');"
  const program = (text: string) =>
    htmlProgram(
      `#html.elem("section", attrs: (id: "panel", "data-origin": "authored"))[#html.elem("span", attrs: (id: "status"))[authored placeholder]]#html.elem("p", attrs: (id: "text"))[${text}]`,
      script,
    )
  const site = await sites.dev({
    initialContent: null,
    beforeStart: (root) => writeFile(join(root, 'site.typ'), program('First')),
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await waitForOpenSocket(page)
  const before = await page.evaluate(() => performance.timeOrigin)
  const navigation = page.waitForEvent('load')
  await writeFile(join(site.root, 'site.typ'), program('Second'))
  await navigation

  await expect(page.locator('#text')).toHaveText('Second')
  await expect(page.locator('#status')).toHaveText('script-ready')
  await expect(page.locator('#panel')).toHaveAttribute('data-origin', 'script-owned')
  expect(await page.evaluate(() => performance.timeOrigin)).not.toBe(before)
})

test('scripted submissions match initialized controls', async ({ page, sites }) => {
  const script =
    "window.appStore = 'seed'; const search = document.getElementById('search'); search.value = window.appStore; search.addEventListener('input', () => { window.appStore = search.value; }); document.getElementById('submit').addEventListener('click', () => { window.submitted = window.appStore; });"
  const program = (text: string) =>
    htmlProgram(
      `#html.elem("input", attrs: (id: "search", value: "seed"))#html.elem("button", attrs: (id: "submit"))[Submit]#html.elem("p", attrs: (id: "text"))[${text}]`,
      script,
    )
  const site = await sites.dev({
    initialContent: null,
    beforeStart: (root) => writeFile(join(root, 'site.typ'), program('First')),
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await waitForOpenSocket(page)
  await page.locator('#search').fill('retained')
  expect(await page.evaluate(() => window.appStore)).toBe('retained')
  const before = await page.evaluate(() => performance.timeOrigin)
  const navigation = page.waitForEvent('load')
  await writeFile(join(site.root, 'site.typ'), program('Second'))
  await navigation
  await page.evaluate(async () => {
    const { promise, resolve } = Promise.withResolvers<void>()
    requestAnimationFrame(() => resolve())
    await promise
  })
  await page.locator('#submit').click()

  const action = await page.evaluate(() => ({
    submitted: window.submitted,
    visible: (document.querySelector('#search') as HTMLInputElement).value,
    store: window.appStore,
  }))
  expect(action).toEqual({ submitted: 'seed', visible: 'seed', store: 'seed' })
  expect(await page.evaluate(() => performance.timeOrigin)).not.toBe(before)
})

for (const scope of ['full response', 'body fragment']) {
  test(`${scope} preserves document runtime ownership`, async ({ page, sites }) => {
    const selector = scope === 'full response' ? 'parsed' : 'parsed.body'
    const loader =
      `window.loadFragment = async () => { const response = await fetch('/fragment.html'); const parsed = new DOMParser().parseFromString(await response.text(), 'text/html'); document.getElementById('slot').replaceChildren(parsed.getElementById('fragment')); for (const original of ${selector}.querySelectorAll('script')) { const script = document.createElement('script'); for (const attribute of original.attributes) script.setAttribute(attribute.name, attribute.value); script.textContent = original.textContent; if (script.src) { const { promise, resolve, reject } = Promise.withResolvers(); script.onload = resolve; script.onerror = reject; document.head.appendChild(script); await promise; } else document.head.appendChild(script); } window.fragmentLoaded = true; }; document.getElementById('load').addEventListener('click', () => window.loadFragment());`
    const site = await sites.dev({
      initialContent: null,
      beforeStart: (root) =>
        writeFile(
          join(root, 'site.typ'),
          htmlProgram(
            '#html.elem("button", attrs: (id: "load"))[Load]#html.elem("section", attrs: (id: "slot"))[Initial]',
            loader,
          ) +
            htmlProgram(
              '#html.elem("section", attrs: (id: "fragment"))[Loaded fragment]',
              'window.fragmentExecutions = (window.fragmentExecutions || 0) + 1;',
              'fragment.html',
            ),
        ),
    })
    await page.goto(site.url, { waitUntil: 'load' })
    await waitForOpenSocket(page)
    await page.evaluate(() => {
      window.initialRuntime = (window as TolaWindow).Tola
      window.initialSocket = window.initialRuntime.ws
    })
    await page.locator('#load').click()
    await page.waitForFunction(() => window.fragmentLoaded === true)

    await expect(page.locator('#slot')).toHaveText('Loaded fragment')
    expect(
      await page.evaluate(() => ({
        sameRuntime: window.initialRuntime === (window as TolaWindow).Tola,
        sameSocket: window.initialSocket === (window as TolaWindow).Tola.ws,
        output: (window as TolaWindow).Tola.activeOutput,
        executions: window.fragmentExecutions,
      })),
    ).toEqual({ sameRuntime: true, sameSocket: true, output: 'index.html', executions: 1 })
  })
}

test('mounted stylesheet updates in place', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: null,
    beforeStart: async (root) => {
      await writeFile(
        join(root, 'site.typ'),
        `#import "@tola/address:0.0.0": asset-url
#document("index.html", format: "html")[
  #html.html(lang: "en")[
    #html.head[#html.link(rel: "stylesheet", href: asset-url("/theme.css"))]
    #html.body[#html.elem("p", attrs: (id: "text"))[First]]
  ]
]
`,
      )
      await mkdir(join(root, 'assets'), { recursive: true })
      await writeFile(join(root, 'assets/theme.css'), '#text { color: rgb(255, 0, 0); }\n')
      const config = await readFile(join(root, 'tola.toml'), 'utf8')
      await writeFile(
        join(root, 'tola.toml'),
        config
          .replace('base-path = "/"', 'base-path = "/文档/"')
          .replace('files = []', 'files = [{ source = "assets/theme.css", url = "/theme.css" }]'),
      )
    },
  })
  await page.goto(new URL(site.url).href, { waitUntil: 'load' })
  await waitForOpenSocket(page)
  const before = await page.evaluate(() => {
    const runtime = window as TolaWindow
    return {
      revision: runtime.Tola.revision,
      timeOrigin: performance.timeOrigin,
      navigations: performance.getEntriesByType('navigation').length,
      color: getComputedStyle(document.querySelector('#text')!).color,
    }
  })

  await writeFile(join(site.root, 'assets/theme.css'), '#text { color: rgb(0, 0, 255); }\n')
  await page.waitForFunction((revision) => {
    const runtime = window as TolaWindow
    return runtime.Tola.revision !== revision
  }, before.revision)

  await expect(page.locator('link[rel~=stylesheet]'))
    .toHaveAttribute('href', /\/%E6%96%87%E6%A1%A3\/theme\.css\?tola-representation=[0-9a-f]{64}$/)
  await expect(page.locator('#text')).toHaveCSS('color', 'rgb(0, 0, 255)')
  expect(before.color).toBe('rgb(255, 0, 0)')
  expect(await page.evaluate(() => performance.timeOrigin)).toBe(before.timeOrigin)
  expect(await page.evaluate(() => performance.getEntriesByType('navigation').length))
    .toBe(before.navigations)
})
