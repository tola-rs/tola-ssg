import { expect, test } from '@playwright/test'
import {
  currentRevision,
  expectReloads,
  installReload,
  modifiedOutput,
  runtimeState,
  sendNextRevision,
} from '../../../support/reload.ts'
import { ONE_PIXEL_PNG } from '../../../support/media.ts'

declare global {
  interface Window {
    sidebar?: Element | null
    clicks?: number
    widgetStore?: { clicks: number; query: string }
    classFrames?: { classes: string; color: string }[]
    sampleClasses?: boolean
  }
}

/** A document whose body content is patched automatically, without any annotation. */
function documentBody(text: string, outside = 'Sidebar', inner = '') {
  return `<!doctype html><html lang="en"><head><title>Document</title></head><body><aside><button id="sidebar">${outside}</button><input id="search" value="seed"></aside><main><p id="text">${text}</p>${inner}</main></body></html>`
}

test('in-place edits preserve page state', async ({ page }) => {
  let body = documentBody('First')
    .replace('<main>', '<main style="height:2400px">')
    .replace('id="sidebar"', 'id="sidebar" title="first"')
  await page.route(
    'https://documents.test/**',
    (route) => route.fulfill({ status: 200, contentType: 'text/html', body }),
  )
  await page.goto('https://documents.test/index.html')
  await installReload(page)
  await page.locator('#search').fill('retained input')
  await page.evaluate(() => {
    const sidebar = document.querySelector('#sidebar')
    if (!sidebar) throw new Error('Sidebar is missing')
    window.sidebar = sidebar
    window.clicks = 0
    sidebar.addEventListener('click', () => {
      window.clicks = (window.clicks ?? 0) + 1
    })
    const search = document.querySelector('#search') as HTMLInputElement
    search.focus()
    search.setSelectionRange(2, 8, 'forward')
    window.scrollTo(0, 600)
  })
  body = documentBody('Second')
    .replace('<main>', '<main style="height:2400px">')
    .replace('id="sidebar"', 'id="sidebar" title="second"')

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('#text')).toHaveText('Second')
  await expect(page.locator('#search')).toHaveValue('retained input')
  expect(await page.evaluate(() => document.querySelector('#sidebar') === window.sidebar)).toBe(true)
  await expect(page.locator('#sidebar')).toHaveAttribute('title', 'second')
  expect(
    await page.evaluate(() => {
      const search = document.querySelector('#search') as HTMLInputElement
      return {
        focus: document.activeElement?.id,
        selection: [search.selectionStart, search.selectionEnd, search.selectionDirection],
        scroll: window.scrollY,
      }
    }),
  ).toEqual({ focus: 'search', selection: [2, 8, 'forward'], scroll: 600 })
  await page.locator('#sidebar').click()
  expect(await page.evaluate(() => window.clicks)).toBe(1)
  await expectReloads(page, 0)
})

test('text edits preserve ongoing scrolling', async ({ page }) => {
  let text = 'First'
  await page.route('https://scrolling-edit.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: documentBody(text).replace('<main>', '<main style="height:6000px">'),
    }))
  await page.goto('https://scrolling-edit.test/index.html', { waitUntil: 'load' })
  await installReload(page)
  await page.evaluate(() => {
    window.scrollTo({ top: 4000, behavior: 'smooth' })
  })
  await page.waitForFunction(() => window.scrollY > 0 && window.scrollY < 4000)
  text = 'Second'

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('#text')).toHaveText('Second')
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(4000)
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
})

test('height edits retain reading position', async ({ page }) => {
  let height = 600
  await page.route('https://reading-position.test/**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        `<!doctype html><html><body><div style="height:${height}px"></div><main style="height:4000px"><p id="reading">Reading</p></main></body></html>`,
    }))
  await page.goto('https://reading-position.test/index.html', { waitUntil: 'load' })
  await installReload(page)
  await page.evaluate(() => {
    const reading = document.querySelector('#reading')!
    window.scrollTo(0, window.scrollY + reading.getBoundingClientRect().top)
  })
  const position = await page.locator('#reading').evaluate((element) => element.getBoundingClientRect().top)
  height = 900

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  expect(await page.locator('#reading').evaluate((element) => element.getBoundingClientRect().top))
    .toBe(position)
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
})

for (const identity of ['id', 'position']) {
  test(`${identity} class updates retain widget state`, async ({ page }) => {
    let classes = 'bg-red-500'
    const { promise: stylesheetReady, resolve: releaseStylesheet } = Promise.withResolvers<void>()
    const script =
      `const store = { clicks: 0, query: 'seed' }; window.widgetStore = store; const widget = document.getElementById('widget'); widget.innerHTML = '<input id="query" value="seed"><button id="toggle">Click</button>'; const query = widget.querySelector('input'); query.addEventListener('input', () => { store.query = query.value; }); widget.querySelector('button').addEventListener('click', () => { store.clicks += 1; });`
    await page.route('https://class-widget.test/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/theme.css') {
        const updated = url.searchParams.has('tola-representation')
        if (updated) await stylesheetReady
        return route.fulfill({
          status: 200,
          contentType: 'text/css',
          body: updated
            ? '.bg-blue-500 { background-color: rgb(0, 0, 255); }'
            : '.bg-red-500 { background-color: rgb(255, 0, 0); }',
        })
      }
      const sourceClasses = route.request().headers()['x-tola-revision'] === '0'.repeat(64)
        ? 'bg-red-500'
        : classes
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body:
          `<!doctype html><html><head><link rel="stylesheet" href="/theme.css"></head><body><main><section ${
            identity === 'id' ? 'id="panel"' : ''
          } class="${sourceClasses}"><h1>Title</h1><div id="widget"></div></section></main><script>${script}</script></body></html>`,
      })
    })
    await page.goto('https://class-widget.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    await page.locator('#toggle').click()
    await page.locator('#query').fill('retained query')
    const widget = await page.locator('#widget').elementHandle()
    await page.evaluate(() => {
      const query = document.querySelector('#query') as HTMLInputElement
      query.setSelectionRange(2, 8, 'forward')
      window.classFrames = []
      window.sampleClasses = true
      const sample = () => {
        const panel = document.querySelector('section')!
        window.classFrames?.push({ classes: panel.className, color: getComputedStyle(panel).backgroundColor })
        if (window.sampleClasses) requestAnimationFrame(sample)
      }
      requestAnimationFrame(sample)
    })
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
    classes = 'bg-blue-500'
    const requested = page.waitForRequest((request) =>
      request.url().includes('/theme.css?tola-representation=')
    )
    const update = sendNextRevision(page, [
      modifiedOutput('index.html', 'html-document'),
      modifiedOutput('theme.css'),
    ])
    try {
      await Promise.race([
        requested,
        update.then(() => {
          throw new Error('Class update completed before its stylesheet was prepared')
        }),
      ])
      await expect(page.locator('section')).toHaveClass('bg-red-500')
      await expect(page.locator('section')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
    } finally {
      releaseStylesheet()
      await update
    }

    await expect(page.locator('section')).toHaveClass('bg-blue-500')
    await expect(page.locator('section')).toHaveCSS('background-color', 'rgb(0, 0, 255)')
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
    await page.evaluate(() => {
      window.sampleClasses = false
    })
    const frames = await page.evaluate(() => window.classFrames ?? [])
    expect(frames).toContainEqual({ classes: 'bg-red-500', color: 'rgb(255, 0, 0)' })
    expect(frames).toContainEqual({ classes: 'bg-blue-500', color: 'rgb(0, 0, 255)' })
    expect(frames.every(({ classes, color }) =>
      (classes === 'bg-red-500' && color === 'rgb(255, 0, 0)') ||
      (classes === 'bg-blue-500' && color === 'rgb(0, 0, 255)')
    )).toBe(true)
    expect(await widget!.evaluate((element) => element === document.querySelector('#widget'))).toBe(true)
    await expect(page.locator('#query')).toHaveValue('retained query')
    expect(
      await page.locator('#query').evaluate((element) => {
        const query = element as HTMLInputElement
        return {
          focused: document.activeElement === query,
          selection: [query.selectionStart, query.selectionEnd, query.selectionDirection],
        }
      }),
    ).toEqual({ focused: true, selection: [2, 8, 'forward'] })
    await page.locator('#toggle').click()
    expect(await page.evaluate(() => window.widgetStore)).toEqual({ clicks: 2, query: 'retained query' })
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
  })
}

for (const conflict of ['class conflict', 'position shift', 'position reorder', 'ambiguous position']) {
  test(`${conflict} rejects class update`, async ({ page }) => {
    let classes = 'bg-red-500'
    const tag = conflict === 'position reorder' || conflict === 'ambiguous position' ? 'p' : 'section'
    const sibling = conflict === 'position reorder'
      ? '<p class="bg-red-500">Other</p>'
      : conflict === 'ambiguous position'
      ? '<p class="bg-red-500">Static</p>'
      : ''
    const script = conflict === 'class conflict'
      ? "document.getElementById('panel').classList.add('runtime-owned');"
      : conflict === 'position shift'
      ? "const inserted = document.createElement('section'); inserted.className = 'bg-red-500'; inserted.textContent = 'Dynamic'; document.querySelector('main').prepend(inserted);"
      : conflict === 'position reorder'
      ? "const main = document.querySelector('main'); main.append(main.firstElementChild);"
      : "window.widgetStore = { clicks: 0, query: 'ready' };"
    await page.route('https://class-conflict.test/**', (route) => {
      const sourceClasses = route.request().headers()['x-tola-revision'] === '0'.repeat(64)
        ? 'bg-red-500'
        : classes
      return route.fulfill({
        status: 200,
        contentType: 'text/html',
        body: `<!doctype html><html><body><main><${tag} ${
          conflict === 'class conflict' ? 'id="panel"' : ''
        } class="${sourceClasses}">Static</${tag}>${sibling}</main><script>${script}</script></body></html>`,
      })
    })
    await page.goto('https://class-conflict.test/index.html', { waitUntil: 'load' })
    await installReload(page)
    const before = await page.locator(tag).evaluateAll((elements) =>
      elements.map((element) => element.className)
    )
    classes = 'bg-blue-500'

    await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

    expect(
      await page.locator(tag).evaluateAll((elements) => elements.map((element) => element.className)),
    )
      .toEqual(before)
    expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
  })
}

test('head metadata patches without reload', async ({ page }) => {
  let document =
    `<!doctype html><html><head><title>First</title><meta name="description" content="first"></head><body><p id="text">Body</p></body></html>`
  await page.route(
    'https://head.test/**',
    (route) => route.fulfill({ status: 200, contentType: 'text/html', body: document }),
  )
  await page.goto('https://head.test/index.html')
  await installReload(page)
  document =
    `<!doctype html><html><head><title>Second</title><meta name="description" content="second"></head><body><p id="text">Body</p></body></html>`

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page).toHaveTitle('Second')
  await expect(page.locator('meta[name="description"]')).toHaveAttribute('content', 'second')
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
})

/** One document edit the runtime cannot patch in place, so it must navigate instead. */
type UnpatchableEdit = {
  /** Names the edit in the case title. */
  name: string
  /** Markup the first document already carries, when the edit needs it present before the update. */
  before?: string
  /** Builds the updated document from its unmodified second revision. */
  after: (document: string) => string
}

const unpatchableEdits: readonly UnpatchableEdit[] = [
  {
    name: 'script',
    after: () => documentBody('Second', 'Sidebar', '<script>window.started = true;</script>'),
  },
  {
    name: 'SVG script',
    after: () => documentBody('Second', 'Sidebar', '<svg><script>window.started = true;</script></svg>'),
  },
  {
    name: 'custom element',
    after: () => documentBody('Second', 'Sidebar', '<live-chart></live-chart>'),
  },
  {
    name: 'inline handler',
    after: (document) => document.replace('id="sidebar"', 'id="sidebar" onclick="void 0"'),
  },
  {
    name: 'JavaScript link',
    before: '<a id="destination" href="/safe">Link</a>',
    after: () => documentBody('Second', 'Sidebar', '<a id="destination" href="javascript:void(0)">Link</a>'),
  },
  {
    name: 'customized element',
    after: (document) => document.replace('id="sidebar"', 'id="sidebar" is="live-button"'),
  },
  {
    name: 'iframe',
    after: () => documentBody('Second', 'Sidebar', '<iframe src="/frame.html"></iframe>'),
  },
  {
    name: 'body attributes',
    after: (document) => document.replace('<body>', '<body class="changed">'),
  },
  {
    name: 'document element',
    after: (document) => document.replace('<html lang="en">', '<html lang="zh">'),
  },
  {
    name: 'head link',
    after: (document) =>
      document.replace('</head>', '<link rel="preload" href="/cover.png" as="image"></head>'),
  },
  {
    name: 'stylesheet link',
    after: (document) => document.replace('</head>', '<link rel="stylesheet" href="/site.css"></head>'),
  },
  {
    name: 'video source',
    before: '<video id="clip" src="/first.mp4"></video>',
    after: () => documentBody('Second', 'Sidebar', '<video id="clip" src="/second.mp4"></video>'),
  },
  {
    name: 'srcset',
    after: () =>
      documentBody(
        'Second',
        'Sidebar',
        '<img id="cover" src="/cover.png" srcset="/cover.png 1x, /cover.png 2x">',
      ),
  },
]

for (const edit of unpatchableEdits) {
  test(`${edit.name} change reloads page`, async ({ page }) => {
    let body = documentBody('First', 'Sidebar', edit.before ?? '')
    await page.route(
      'https://unsupported.test/**',
      (route) => route.fulfill({ status: 200, contentType: 'text/html', body }),
    )
    await page.goto('https://unsupported.test/index.html')
    await installReload(page)
    body = edit.after(documentBody('Second'))

    await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

    await expectReloads(page, 1)
    await expect(page.locator('#text')).toHaveText('First')
    expect(await currentRevision(page)).toBe('0'.repeat(64))
  })
}

for (const type of ['application/json', 'application/ld+json']) {
  test(`${type} data retains in-place edits`, async ({ page }) => {
    let body = documentBody('First', 'Sidebar', `<script type="${type}">{"value":1}</script>`)
    await page.route(
      'https://data-blocks.test/**',
      (route) => route.fulfill({ status: 200, contentType: 'text/html', body }),
    )
    await page.goto('https://data-blocks.test/index.html')
    await installReload(page)
    body = documentBody('Second', 'Sidebar', `<script type="${type}">{"value":2}</script>`)

    await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

    await expect(page.locator('#text')).toHaveText('Second')
    expect(await page.locator(`script[type="${type}"]`).textContent()).toBe('{"value":2}')
    expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
  })
}

for (const behavior of ['external data block', 'removed data source', 'SVG data block']) {
  test(`${behavior} requires navigation`, async ({ page }) => {
    const inner = behavior === 'SVG data block'
      ? '<svg><script type="application/json">{"value":1}</script></svg>'
      : `<script type="application/json"${
        behavior === 'external data block' ? ' src="/settings.json"' : ''
      }>{"value":1}</script>`
    let body = documentBody('First', 'Sidebar', inner)
    await page.route(
      'https://data-behavior.test/**',
      (route) => route.fulfill({ status: 200, contentType: 'text/html', body }),
    )
    await page.goto('https://data-behavior.test/index.html')
    await installReload(page)
    if (behavior === 'removed data source') {
      await page.evaluate(() => {
        const script = document.querySelector('script[type="application/json"]')!
        script.setAttribute('src', '/settings.json')
        script.removeAttribute('src')
      })
    }
    body = documentBody('Second', 'Sidebar', inner)

    await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

    await expect(page.locator('#text')).toHaveText('First')
    expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
  })
}

test('superseded update never commits', async ({ page }) => {
  await page.route(
    'https://fenced.test/**',
    (route) =>
      route.fulfill(
        route.request().url().includes('tola-representation')
          ? { status: 409, contentType: 'text/plain', body: 'site changed; reload required' }
          : { status: 200, contentType: 'text/html', body: documentBody('First') },
      ),
  )
  await page.goto('https://fenced.test/index.html')
  await installReload(page)
  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])
  await expect(page.locator('#text')).toHaveText('First')
  await expectReloads(page, 1)
})

test('page update awaits content images', async ({ page }) => {
  let body = documentBody('First')
  let releaseImage!: () => void
  const released = new Promise<void>((resolve) => {
    releaseImage = resolve
  })
  await page.route('https://prepared.test/**', async (route) => {
    if (new URL(route.request().url()).pathname === '/cover.png') {
      await released
      return route.fulfill({ status: 200, contentType: 'image/png', body: ONE_PIXEL_PNG })
    }
    return route.fulfill({ status: 200, contentType: 'text/html', body })
  })
  await page.goto('https://prepared.test/index.html')
  await installReload(page)
  body = documentBody('Second', 'Sidebar', '<img id="cover" src="/cover.png" loading="lazy">')
  const requested = page.waitForRequest((request) => new URL(request.url()).pathname === '/cover.png')
  const update = sendNextRevision(page, [
    modifiedOutput('index.html', 'html-document'),
    modifiedOutput('cover.png'),
  ])
  try {
    const prematureUpdate = update.then(() => {
      throw new Error('Document update completed before its image was prepared')
    })
    await Promise.race([requested, prematureUpdate])
    await expect(page.locator('#text')).toHaveText('First')
  } finally {
    releaseImage()
    await update
  }
  await expect(page.locator('#text')).toHaveText('Second')
  expect(await page.locator('#cover').evaluate((image) => (image as HTMLImageElement).complete)).toBe(true)
  await expect(page.locator('#cover')).toHaveAttribute('loading', 'lazy')
  await expectReloads(page, 0)
})

test('base-resolved image survives updates', async ({ page }) => {
  let text = 'First'
  await page.route('https://based-document.test/**', (route) => {
    const url = new URL(route.request().url())
    if (url.pathname.endsWith('/cover.svg')) {
      const width = url.pathname === '/cover.svg' ? 30 : url.searchParams.has('tola-representation') ? 20 : 10
      return route.fulfill({
        status: 200,
        contentType: 'image/svg+xml',
        body:
          `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="10"><rect width="100%" height="100%" fill="red"/></svg>`,
      })
    }
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body:
        `<!doctype html><html><head><base href="/images/"></head><body><main><p id="text">${text}</p><img id="cover" src="cover.svg"></main></body></html>`,
    })
  })
  await page.goto('https://based-document.test/index.html', { waitUntil: 'load' })
  await installReload(page)
  text = 'Second'

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document')])

  await expect(page.locator('#text')).toHaveText('Second')
  expect(
    await page.locator('#cover').evaluate((image) => ({
      src: (image as HTMLImageElement).currentSrc,
      width: (image as HTMLImageElement).naturalWidth,
    })),
  ).toEqual({ src: 'https://based-document.test/images/cover.svg', width: 10 })

  text = 'Third'
  await sendNextRevision(page, [
    modifiedOutput('index.html', 'html-document', 'c'.repeat(64)),
    modifiedOutput('images/cover.svg', 'asset', 'c'.repeat(64)),
  ])

  await expect(page.locator('#text')).toHaveText('Third')
  expect(
    await page.locator('#cover').evaluate((image) => ({
      src: (image as HTMLImageElement).currentSrc,
      width: (image as HTMLImageElement).naturalWidth,
    })),
  ).toEqual({
    src: `https://based-document.test/images/cover.svg?tola-representation=${'c'.repeat(64)}`,
    width: 20,
  })
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'c'.repeat(64) })
})

test('failed image preparation discards update', async ({ page }) => {
  let body = documentBody('First')
  await page.route('https://failed-image.test/**', (route) => {
    if (new URL(route.request().url()).pathname === '/cover.png') return route.abort()
    return route.fulfill({ status: 200, contentType: 'text/html', body })
  })
  await page.goto('https://failed-image.test/index.html')
  await installReload(page)
  const main = await page.locator('main').elementHandle()
  body = documentBody('Second', 'Sidebar', '<img id="cover" src="/cover.png">')

  await sendNextRevision(page, [modifiedOutput('index.html', 'html-document'), modifiedOutput('cover.png')])

  await expect(page.locator('#text')).toHaveText('First')
  await expect(page.locator('#cover')).toHaveCount(0)
  expect(await main!.evaluate((element) => element === document.querySelector('main'))).toBe(true)
  expect(await runtimeState(page)).toEqual({ reloads: 1, revision: '0'.repeat(64) })
})

test('image address edits await replacement', async ({ page }) => {
  let imageName = 'first.svg'
  const { promise: released, resolve: releaseImage } = Promise.withResolvers<void>()
  await page.route('https://changed-image.test/**', async (route) => {
    const url = new URL(route.request().url())
    if (url.pathname.endsWith('.svg')) {
      if (url.pathname === '/images/second.svg') await released
      return route.fulfill({
        status: 200,
        contentType: 'image/svg+xml',
        body: `<svg xmlns="http://www.w3.org/2000/svg" width="${
          url.pathname.endsWith('second.svg') ? 20 : 10
        }" height="10"/>`,
      })
    }
    return route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: documentBody(imageName, 'Sidebar', `<img id="cover" src="${imageName}" loading="lazy">`)
        .replace('</head>', '<base href="/images/"></head>'),
    })
  })
  await page.goto('https://changed-image.test/index.html', { waitUntil: 'load' })
  await installReload(page)
  const original = await page.locator('#cover').elementHandle()
  await page.locator('#search').fill('retained input')
  imageName = 'second.svg'
  const requested = page.waitForRequest((request) => new URL(request.url()).pathname === '/images/second.svg')
  const update = sendNextRevision(page, [
    modifiedOutput('index.html', 'html-document'),
    modifiedOutput('images/first.svg'),
    modifiedOutput('images/second.svg'),
  ])
  try {
    const prematureUpdate = update.then(() => {
      throw new Error('Image address update completed before its replacement was prepared')
    })
    await Promise.race([requested, prematureUpdate])
    await expect(page.locator('#text')).toHaveText('first.svg')
    await expect(page.locator('#cover')).toHaveAttribute('src', 'first.svg')
  } finally {
    releaseImage()
    await update
  }

  await expect(page.locator('#text')).toHaveText('second.svg')
  await expect(page.locator('#search')).toHaveValue('retained input')
  await expect(page.locator('#cover')).toHaveAttribute('loading', 'lazy')
  expect(await original!.evaluate((image) => image === document.querySelector('#cover'))).toBe(true)
  await expect.poll(() =>
    page.locator('#cover').evaluate((image) => ({
      src: (image as HTMLImageElement).currentSrc,
      width: (image as HTMLImageElement).naturalWidth,
    }))
  ).toEqual({
    src: `https://changed-image.test/images/second.svg?tola-representation=${'b'.repeat(64)}`,
    width: 20,
  })
  expect(await runtimeState(page)).toEqual({ reloads: 0, revision: 'b'.repeat(64) })
})
