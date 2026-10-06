import { expect } from '@playwright/test'
import { mkdir, readFile, rm, stat, unlink, writeFile } from 'node:fs/promises'
import type { ServerResponse } from 'node:http'
import { isAbsolute, join, relative } from 'node:path'
import { startGate } from '../../support/gate.ts'
import {
  hookJournal,
  hookToml,
  readHookJournal,
  writeHookConfiguration,
  writeUnreadContentFile,
} from '../../support/hooks.ts'
import { readSessionRecords } from '../../support/log.ts'
import { currentRevision, waitForOpenSocket } from '../../support/reload.ts'
import { test } from '../../support/server.ts'
import { RENDER_EVERY_SOURCE, writeSiteProgram } from '../../support/site.ts'

type HookJournalRecord = 'cache' | 'reuse' | 'convert'

/** The image hook's journal holds `cache <dir>` once per resolved directory, then one
 *  `reuse <source>` or `convert <source> <pair>` record per input on each run. */
function journalRecords(journal: string, record: HookJournalRecord): string[] {
  const prefix = `${record} `
  return journal.split('\n').filter((line) => line.startsWith(prefix)).map((line) =>
    line.slice(prefix.length)
  )
}

async function writeSearchIndexHook(root: string): Promise<string> {
  const script = join(root, 'index-search.mjs')
  await writeFile(
    script,
    `
    import { mkdir, readFile, writeFile } from 'node:fs/promises';
    import { join } from 'node:path';
    const html = await readFile(join(process.env.TOLA_HOOK_INPUT_DIR, 'article/index.html'), 'utf8');
    const heading = html.match(/<h1(?:\\s[^>]*)?>([\\s\\S]*?)<\\/h1>/)?.[1];
    if (!heading) throw new Error('The article has no search title');
    const title = heading.replace(/<[^>]+>/g, '').trim();
    const output = join(process.env.TOLA_HOOK_OUTPUT_DIR, 'search');
    await mkdir(output, { recursive: true });
    await writeFile(join(output, 'index.json'), JSON.stringify([{ title, url: '/article/' }]));
  `,
  )
  return hookToml('generate-outputs', script, { name: 'search', outputs: [{ file: 'search/index.json' }] })
}

test('generated styles preserve active controls', async ({ page, sites }) => {
  const contentWithClass = (className: string) =>
    `#html.elem("main", attrs: (id: "card", class: ${JSON.stringify(className)}))[Stable content]\n`
  const styles = (color: string) => `.brand { background-color: ${color}; }\n`
  const widget = `
    const search = document.querySelector('#search');
    const searchInput = document.createElement('input');
    const results = document.createElement('section');
    search.append(searchInput, results);
    searchInput.addEventListener('input', () => {
      results.textContent = searchInput.value.includes('searchable') ? 'Searchable content' : '';
    });
    window.searchUI = { searchInput, results };
  `
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: contentWithClass('brand') },
    beforeStart: async (root) => {
      await mkdir(join(root, 'static/styles'), { recursive: true })
      await mkdir(join(root, 'static/web-assets/generated-styles'), { recursive: true })
      await writeFile(join(root, 'static/styles/site.css'), styles('#ff0000'))
      const script = join(root, 'generate-styles.mjs')
      await writeFile(
        script,
        `
        import { readFile, writeFile } from 'node:fs/promises';
        const stylesheet = await readFile('static/styles/site.css', 'utf8');
        const source = await readFile('content/index.typ', 'utf8');
        const accent = source.includes('class: "accent"') ? '\\n.accent { background-color: #00ff00; }\\n' : '';
        await writeFile('static/web-assets/generated-styles/site.css', stylesheet + accent);
      `,
      )
      await writeSiteProgram(
        root,
        `#document("index.html", format: "html")[#html.html[
        #html.head[#html.link(rel: "stylesheet", href: "/assets/generated-styles/site.css")]
        #html.body[
          #include "content/index.typ"
          #html.elem("div", attrs: (id: "search"))[]
          #html.script(${JSON.stringify(widget)})
        ]
      ]]\n`,
      )
      await writeHookConfiguration(root, [hookToml('before-build', script, {
        name: 'styles',
        outputs: ['static/web-assets/generated-styles'],
        rerunOn: ['static/styles'],
      })], '[assets]\ntrees = [{ source = "static/web-assets", url-prefix = "/assets/" }]\n')
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await waitForOpenSocket(page)
  await expect(page.locator('#card')).toHaveCSS('background-color', 'rgb(255, 0, 0)')
  const searchInput = page.locator('#search input')
  await searchInput.fill('searchable')
  await expect(page.locator('#search section')).toHaveText('Searchable content')
  const timeOrigin = await page.evaluate(() => {
    const browser = window as typeof window & {
      searchUI: unknown
      retainedSearch: unknown
      retainedInput: unknown
      styleFrames: string[]
      styleSampling: boolean
    }
    browser.retainedSearch = browser.searchUI
    browser.retainedInput = document.querySelector('#search input')
    browser.styleFrames = []
    browser.styleSampling = true
    const capture = () => {
      const card = document.querySelector('#card')!
      browser.styleFrames.push(JSON.stringify([card.className, getComputedStyle(card).backgroundColor]))
      if (browser.styleSampling) requestAnimationFrame(capture)
    }
    requestAnimationFrame(capture)
    return performance.timeOrigin
  })
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  await writeFile(join(site.root, 'static/styles/site.css'), styles('#0000ff'))
  await expect(page.locator('#card')).toHaveCSS('background-color', 'rgb(0, 0, 255)')
  await expect(searchInput).toHaveValue('searchable')
  await expect(searchInput).toBeFocused()
  expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin)

  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  await site.writeContent('index.typ', contentWithClass('accent'))
  await expect(page.locator('#card')).toHaveCSS('background-color', 'rgb(0, 255, 0)')
  await expect(searchInput).toHaveValue('searchable')
  await expect(searchInput).toBeFocused()
  expect(
    await page.evaluate(() => {
      const browser = window as typeof window & {
        searchUI: unknown
        retainedSearch: unknown
        retainedInput: unknown
      }
      return {
        origin: performance.timeOrigin,
        sameUI: browser.searchUI === browser.retainedSearch,
        sameInput: document.querySelector('#search input') === browser.retainedInput,
      }
    }),
  ).toEqual({ origin: timeOrigin, sameUI: true, sameInput: true })
  await expect(page.locator('#search section')).toHaveText('Searchable content')
  await page.evaluate(() =>
    new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    })
  )
  const frames = await page.evaluate(() => {
    const browser = window as typeof window & { styleFrames: string[]; styleSampling: boolean }
    browser.styleSampling = false
    return browser.styleFrames
  })
  expect(frames.length).toBeGreaterThan(0)
  expect(new Set(frames)).toEqual(
    new Set([
      '["brand","rgb(255, 0, 0)"]',
      '["brand","rgb(0, 0, 255)"]',
      '["accent","rgb(0, 255, 0)"]',
    ]),
  )
})

test('generated index preserves active search', async ({ page, sites }) => {
  const search = `
    (async () => {
      const searchInput = document.querySelector('#search');
      const results = document.querySelector('#results');
      let searchRevision = 0;
      let cachedIndex = null;
      const match = async term => {
        cachedIndex ??= fetch('/search/index.json', { cache: 'no-store' }).then(response => {
          if (!response.ok) throw new Error('The search index could not be loaded');
          return response.json();
        });
        const pages = await cachedIndex;
        return pages.filter(page => page.title.toLowerCase().includes(term.toLowerCase())).map(page => {
          const link = document.createElement('a');
          link.href = page.url;
          link.textContent = page.title;
          return link;
        });
      };
      window.searchUI = { searchInput, results };
      const renderSearch = async signal => {
        const revision = ++searchRevision;
        const term = searchInput.value;
        const links = await match(term);
        signal?.throwIfAborted();
        if (revision === searchRevision && searchInput.value === term) results.replaceChildren(...links);
      };
      searchInput.addEventListener('input', () => renderSearch());
      await renderSearch();
      window.addEventListener('tola:before-update', event => {
        const changed = event.detail.changes.filter(change => change.path === 'search/index.json');
        if (!changed.length) return;
        event.detail.accept(changed.map(change => change.path), async ({ signal }) => {
          signal.throwIfAborted();
          searchRevision++;
          cachedIndex = null;
          await renderSearch(signal);
        });
      });
    })();
  `
  const site = await sites.dev({
    initialContent: { relativePath: 'article.typ', source: '#html.h1[Quartz topic]\nFirst topic content.\n' },
    beforeStart: async (root) => {
      await writeSiteProgram(
        root,
        `#document("index.html", format: "html")[#html.html[
        #html.head[]#html.body[
          #html.input(id: "search", value: "topic")
          #html.elem("section", attrs: (id: "results"))[]
          #html.script(${JSON.stringify(search)})
        ]
      ]]
      #document("article/index.html", format: "html")[#html.html[
        #html.head[]#html.body[#html.elem("main")[#include "content/article.typ"]]
      ]]\n`,
      )
      await writeHookConfiguration(root, [await writeSearchIndexHook(root)])
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await waitForOpenSocket(page)
  const searchInput = page.locator('#search')
  await searchInput.focus()
  await expect(page.locator('#results a')).toHaveText('Quartz topic')
  const first = await currentRevision(page)
  const timeOrigin = await page.evaluate(() => {
    const browser = window as typeof window & {
      searchUI: unknown
      retainedSearch: unknown
      retainedInput: unknown
    }
    browser.retainedSearch = browser.searchUI
    browser.retainedInput = document.querySelector('#search')
    return performance.timeOrigin
  })
  await site.writeContent('article.typ', '#html.h1[Volcano topic]\nSecond topic content.\n')
  await expect(page.locator('#results a')).toHaveText('Volcano topic')
  await expect.poll(() => currentRevision(page)).not.toBe(first)
  await expect(searchInput).toHaveValue('topic')
  await expect(searchInput).toBeFocused()
  expect(
    await page.evaluate(() => {
      const browser = window as typeof window & {
        searchUI: unknown
        retainedSearch: unknown
        retainedInput: unknown
      }
      return {
        origin: performance.timeOrigin,
        sameUI: browser.searchUI === browser.retainedSearch,
        sameInput: document.querySelector('#search') === browser.retainedInput,
      }
    }),
  ).toEqual({ origin: timeOrigin, sameUI: true, sameInput: true })
})

test('generated images publish latest sources', async ({ page, sites, directory }) => {
  const sourceCount = 300
  const names = [
    'cover',
    ...Array.from({ length: sourceCount - 1 }, (_, index) => `image-${String(index).padStart(3, '0')}`),
  ]
  // Known encoded pairs isolate hook publication from codec behavior.
  const images = await page.evaluate(async () => {
    const encoded: { name: string; jpeg: string; webp: string; pixel: number[] }[] = []
    for (
      const [name, color] of ([['red', '#ff0000'], ['blue', '#0000ff'], ['green', '#00ff00'], [
        'yellow',
        '#ffff00',
      ]] as const)
    ) {
      const canvas = document.createElement('canvas')
      canvas.width = 16
      canvas.height = 16
      const paint = canvas.getContext('2d')!
      paint.fillStyle = color
      paint.fillRect(0, 0, 16, 16)
      const jpeg = canvas.toDataURL('image/jpeg', 1)
      const webp = canvas.toDataURL('image/webp', 1)
      const decoded = await createImageBitmap(await (await fetch(webp)).blob())
      paint.drawImage(decoded, 0, 0)
      encoded.push({
        name,
        jpeg: jpeg.slice(jpeg.indexOf(',') + 1),
        webp: webp.slice(webp.indexOf(',') + 1),
        pixel: Array.from(paint.getImageData(8, 8, 1, 1).data),
      })
      decoded.close()
    }
    return encoded
  })
  const [red, blue, green, yellow] = [images[0]!, images[1]!, images[2]!, images[3]!]
  const journal = join(directory, 'image-conversions.txt')
  let holdConversions = false
  const held = new Set<ServerResponse>()
  const gate = await startGate((_request, response) => {
    if (holdConversions) {
      held.add(response)
      response.once('close', () => held.delete(response))
    } else response.end('continue')
  })
  type ImageChange =
    | { operation: 'modified'; path: string; before: string; after: string }
    | { operation: 'added' | 'removed'; path: string; representation: string }
  const imageRevisions: { from: string; to: string; changes: ImageChange[] }[] = []
  let overflowedImageRevisions = false
  page.on('websocket', (socket) =>
    socket.on('framereceived', (frame) => {
      const message = JSON.parse(String(frame.payload))
      const changes: ImageChange[] = []
      for (const change of message.diff?.changes ?? []) {
        const output = change.operation === 'modified' ? change.output.after : change.output
        if (!output.path.startsWith('assets/images/')) continue
        if (changes.length === sourceCount) {
          overflowedImageRevisions = true
          return
        }
        changes.push(
          change.operation === 'modified'
            ? {
              operation: 'modified',
              path: output.path,
              before: change.output.before.representation,
              after: output.representation,
            }
            : { operation: change.operation, path: output.path, representation: output.representation },
        )
      }
      if (changes.length) {
        if (imageRevisions.length === 64) {
          overflowedImageRevisions = true
          return
        }
        imageRevisions.push({ from: message.diff.from, to: message.diff.to, changes })
      }
    }))
  const writeSources = async (root: string, image: typeof red) => {
    for (let offset = 0; offset < names.length; offset += 16) {
      await Promise.all(
        names.slice(offset, offset + 16).map((name) =>
          writeFile(join(root, 'static/image-sources', `${name}.jpg`), Buffer.from(image.jpeg, 'base64'))
        ),
      )
    }
  }
  try {
    const site = await sites.dev({
      initialContent: { relativePath: 'index.typ', source: 'Generated image page.\n' },
      beforeStart: async (root) => {
        await mkdir(join(root, 'static/image-sources'), { recursive: true })
        await mkdir(join(root, 'static/web-assets'), { recursive: true })
        await rm(join(root, 'static/web-assets/images'), { recursive: true, force: true })
        await expect(stat(join(root, 'static/web-assets/images'))).rejects.toMatchObject({ code: 'ENOENT' })
        await writeSources(root, red)
        const script = join(root, 'generate-images.mjs')
        await writeFile(
          script,
          `
          import { appendFile, mkdir, readFile, readdir, rename, stat, writeFile } from 'node:fs/promises';
          import { join } from 'node:path';
          const pairs = ${JSON.stringify(images)};
          const cache = process.env.TOLA_HOOK_CACHE_DIR;
          const journal = ${JSON.stringify(journal)};
          const pending = [];
          const record = (entry) => appendFile(journal, entry + '\\n');
          const recorded = await readFile(journal, 'utf8').catch(error => { if (error.code === 'ENOENT') return ''; throw error; });
          if (!recorded.includes('cache ' + cache + '\\n')) await record('cache ' + cache);
          for (const name of (await readdir('static/image-sources')).filter(name => name.endsWith('.jpg')).sort()) {
            const source = await readFile(join('static/image-sources', name));
            const output = join('static/web-assets/images', name.replace(/\\.jpg$/, '.webp'));
            const cached = await readFile(join(cache, name)).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
            const exists = await stat(output).then(file => file.isFile()).catch(error => { if (error.code === 'ENOENT') return false; throw error; });
            if (exists && cached?.equals(source)) {
              await record('reuse ' + name);
              continue;
            }
            const pair = pairs.find(pair => Buffer.from(pair.jpeg, 'base64').equals(source));
            if (!pair) throw new Error('Image input needs repair');
            pending.push({ name, source, output, pair });
          }
          if (pending.length) {
            const response = await fetch(${JSON.stringify(gate.url)});
            await response.text();
            await mkdir('static/web-assets/images', { recursive: true });
            await mkdir(cache, { recursive: true });
          }
          for (const image of pending) {
            await writeFile(image.output + '.tmp', Buffer.from(image.pair.webp, 'base64'));
            await rename(image.output + '.tmp', image.output);
            await writeFile(join(cache, image.name), image.source);
            await record('convert ' + image.name + ' ' + image.pair.name);
          }
        `,
        )
        await writeSiteProgram(
          root,
          `#document("index.html", format: "html")[#html.html[
          #html.head[]#html.body[
            #include "content/index.typ"
            #html.img(src: "/assets/images/cover.webp", id: "cover", width: 16, height: 16, decoding: "async")
            #html.input(id: "search", value: "seed")
          ]
        ]]\n`,
        )
        await writeHookConfiguration(root, [hookToml('before-build', script, {
          name: 'images',
          outputs: ['static/web-assets/images'],
          rerunOn: ['static/image-sources'],
        })], '[assets]\ntrees = [{ source = "static/web-assets", url-prefix = "/assets/" }]\n')
      },
    })
    await page.goto(site.url, { waitUntil: 'load' })
    await waitForOpenSocket(page)
    const cover = page.locator('#cover')
    const pixel = () =>
      cover.evaluate(async (image) => {
        const native = image as HTMLImageElement
        await native.decode()
        const canvas = document.createElement('canvas')
        canvas.width = 16
        canvas.height = 16
        const paint = canvas.getContext('2d')!
        paint.drawImage(native, 0, 0)
        return {
          width: native.naturalWidth,
          height: native.naturalHeight,
          pixel: Array.from(paint.getImageData(8, 8, 1, 1).data),
        }
      })
    expect(await pixel()).toEqual({ width: 16, height: 16, pixel: red.pixel })
    const asset = (name = 'cover', representation?: string) => {
      const url = new URL(`/assets/images/${name}.webp`, site.url)
      if (representation) url.searchParams.set('tola-representation', representation)
      return url.href
    }
    const expectOutput = async (image: typeof red, name = 'cover', representation?: string) => {
      const response = await page.request.get(asset(name, representation))
      expect(response.status()).toBe(200)
      expect(response.headers()['content-type']).toBe('image/webp')
      expect(await response.body()).toEqual(Buffer.from(image.webp, 'base64'))
    }
    for (let offset = 0; offset < names.length; offset += 16) {
      await Promise.all(names.slice(offset, offset + 16).map((name) => expectOutput(red, name)))
    }
    const firstJournal = await readHookJournal(journal)
    const firstConversions = journalRecords(firstJournal, 'convert')
    const cacheRecords = journalRecords(firstJournal, 'cache')
    expect(cacheRecords).toHaveLength(1)
    const cacheDir = cacheRecords[0]!
    expect(isAbsolute(cacheDir)).toBe(true)
    // The cache is site-internal storage, never part of the published assets tree.
    expect(relative(join(site.root, 'static/web-assets'), cacheDir).startsWith('..')).toBe(true)
    const searchInput = page.locator('#search')
    await searchInput.fill('preserved')
    const timeOrigin = await page.evaluate(() => {
      const browser = window as typeof window & {
        retainedImage: unknown
        retainedInput: unknown
        imageFrames: number[][]
        invalidImageFrame: boolean
        captureImages: boolean
      }
      browser.retainedImage = document.querySelector('#cover')
      browser.retainedInput = document.querySelector('#search')
      browser.imageFrames = []
      browser.invalidImageFrame = false
      browser.captureImages = true
      const canvas = document.createElement('canvas')
      canvas.width = 16
      canvas.height = 16
      const paint = canvas.getContext('2d')!
      const capture = () => {
        const image = document.querySelector('#cover') as HTMLImageElement
        if (image.complete && image.naturalWidth === 16 && image.naturalHeight === 16) {
          paint.drawImage(image, 0, 0)
          if (browser.imageFrames.length < 2048) {
            browser.imageFrames.push(Array.from(paint.getImageData(8, 8, 1, 1).data))
          }
        } else browser.invalidImageFrame = true
        if (browser.captureImages) requestAnimationFrame(capture)
      }
      requestAnimationFrame(capture)
      return performance.timeOrigin
    })
    const reusesBeforeEdit = journalRecords(await readHookJournal(journal), 'reuse').length
    await site.writeContent('index.typ', 'Updated image page.\n')
    await expect(page.locator('body')).toContainText('Updated image page.')
    // The edit reruns the hook, which reuses every cached conversion and reconverts none.
    await expect.poll(async () => journalRecords(await readHookJournal(journal), 'reuse').length)
      .toBe(reusesBeforeEdit + names.length)
    expect(journalRecords(await readHookJournal(journal), 'convert')).toEqual(firstConversions)

    const source = join(site.root, 'static/image-sources/cover.jpg')
    const reusesBeforeBlue = journalRecords(await readHookJournal(journal), 'reuse').length
    await writeFile(source, Buffer.from(blue.jpeg, 'base64'))
    await expect.poll(pixel).toEqual({ width: 16, height: 16, pixel: blue.pixel })
    await expect(cover).toHaveAttribute('src', /tola-representation=[a-f0-9]{64}/)
    await expectOutput(blue)
    const blueJournal = await readHookJournal(journal)
    const blueConversions = journalRecords(blueJournal, 'convert')
    expect(blueConversions).toEqual([...firstConversions, 'cover.jpg blue'])
    expect(journalRecords(blueJournal, 'reuse').length).toBe(reusesBeforeBlue + names.length - 1)
    await unlink(join(site.root, 'static/web-assets/images/image-000.webp'))
    await expect.poll(async () => journalRecords(await readHookJournal(journal), 'convert'))
      .toEqual([...blueConversions, 'image-000.jpg red'])
    await expectOutput(red, 'image-000')
    const coverChanges = () =>
      imageRevisions.flatMap((revision) =>
        revision.changes.filter(
          (change): change is Extract<ImageChange, { operation: 'modified' }> =>
            change.operation === 'modified' && change.path === 'assets/images/cover.webp',
        )
      )
    await expect.poll(() => coverChanges().length).toBeGreaterThan(0)
    const allowed = new Set([coverChanges()[0]!.before, coverChanges()[0]!.after])
    const beforePressure = imageRevisions.length
    const blueRevision = await currentRevision(page)
    holdConversions = true
    await writeFile(source, Buffer.from(green.jpeg, 'base64'))
    await expect.poll(() => held.size).toBeGreaterThan(0)
    expect(await currentRevision(page)).toBe(blueRevision)
    await expectOutput(blue)
    expect(await pixel()).toEqual({ width: 16, height: 16, pixel: blue.pixel })
    await writeSources(site.root, yellow)
    await writeFile(source, Buffer.from(red.jpeg, 'base64'))
    holdConversions = false
    for (const response of held) response.end('continue')
    await expect.poll(pixel).toEqual({ width: 16, height: 16, pixel: red.pixel })
    const latestRevision = await currentRevision(page)
    await expect.poll(() => imageRevisions.length).toBeGreaterThan(beforePressure)
    expect(overflowedImageRevisions).toBe(false)
    const pressureRevisions = imageRevisions.slice(beforePressure)
    expect(pressureRevisions).toHaveLength(1)
    const pressure = pressureRevisions[0]!
    expect(pressure.from).toBe(blueRevision)
    expect(pressure.to).toBe(latestRevision)
    expect(pressure.changes.map((change) => change.path).sort()).toEqual(
      names.map((name) => `assets/images/${name}.webp`).sort(),
    )
    expect(pressure.changes.every((change) => change.operation === 'modified')).toBe(true)
    for (let offset = 0; offset < pressure.changes.length; offset += 16) {
      await Promise.all(
        pressure.changes.slice(offset, offset + 16).map((change) => {
          expect(change.operation).toBe('modified')
          const name = change.path.slice('assets/images/'.length, -'.webp'.length)
          return expectOutput(
            name === 'cover' ? red : yellow,
            name,
            change.operation === 'modified' ? change.after : undefined,
          )
        }),
      )
    }
    for (const change of coverChanges()) expect(allowed.has(change.after)).toBe(true)
    await writeFile(source, 'unsupported image bytes')
    await expect.poll(site.stderr).toContain('Image input needs repair')
    expect(await currentRevision(page)).toBe(latestRevision)
    await expectOutput(red)
    const failedConversions = journalRecords(await readHookJournal(journal), 'convert')
    const reusesAfterFailure = journalRecords(await readHookJournal(journal), 'reuse').length
    // Restoring the bytes the failed run refused proves the failure left the persisted cache
    // readable: the cover is reused from it instead of converted again.
    await writeFile(source, Buffer.from(red.jpeg, 'base64'))
    await expect.poll(async () => journalRecords(await readHookJournal(journal), 'reuse').length)
      .toBeGreaterThanOrEqual(reusesAfterFailure + names.length)
    expect(journalRecords(await readHookJournal(journal), 'convert')).toEqual(failedConversions)
    await writeFile(source, Buffer.from(blue.jpeg, 'base64'))
    await expect.poll(pixel).toEqual({ width: 16, height: 16, pixel: blue.pixel })
    await expectOutput(blue)
    await expect(searchInput).toHaveValue('preserved')
    await expect(searchInput).toBeFocused()
    expect(
      await page.evaluate(() => {
        const browser = window as typeof window & {
          retainedImage: unknown
          retainedInput: unknown
          imageFrames: number[][]
          invalidImageFrame: boolean
          captureImages: boolean
        }
        browser.captureImages = false
        return {
          origin: performance.timeOrigin,
          sameImage: document.querySelector('#cover') === browser.retainedImage,
          sameInput: document.querySelector('#search') === browser.retainedInput,
          frames: browser.imageFrames,
          invalidFrame: browser.invalidImageFrame,
        }
      }),
    ).toEqual({
      origin: timeOrigin,
      sameImage: true,
      sameInput: true,
      frames: expect.arrayContaining([red.pixel, blue.pixel]),
      invalidFrame: false,
    })
    const frames = await page.evaluate(() =>
      (window as typeof window & { imageFrames: number[][] }).imageFrames
    )
    expect(
      frames.every((frame) =>
        JSON.stringify(frame) === JSON.stringify(red.pixel) ||
        JSON.stringify(frame) === JSON.stringify(blue.pixel)
      ),
    ).toBe(true)
    const completedJournal = await readHookJournal(journal)
    const completedConversions = journalRecords(completedJournal, 'convert')
    const completedReuses = journalRecords(completedJournal, 'reuse').length
    await site.restart()
    await expectOutput(blue)
    // The restart runs the hook again: every conversion is reused from the same cache directory.
    await expect.poll(async () => journalRecords(await readHookJournal(journal), 'reuse').length)
      .toBe(completedReuses + names.length)
    expect(journalRecords(await readHookJournal(journal), 'convert')).toEqual(completedConversions)
    expect(journalRecords(await readHookJournal(journal), 'cache')).toEqual([cacheDir])

    // Clearing the persisted cache makes the next build regenerate it: every image is converted
    // again from its source, and a further edit reuses the regenerated cache without another
    // reuse pass, which cache writes of the regeneration would otherwise schedule.
    const conversionsBeforeClear = journalRecords(await readHookJournal(journal), 'convert')
    const reusesBeforeClear = journalRecords(await readHookJournal(journal), 'reuse').length
    await rm(cacheDir, { recursive: true, force: true })
    await site.writeContent('index.typ', 'Cleared image cache page.\n')
    await expect(page.locator('body')).toContainText('Cleared image cache page.')
    const regenerated = names.map((name) => `${name}.jpg ${name === 'cover' ? 'blue' : 'yellow'}`)
    await expect.poll(async () => journalRecords(await readHookJournal(journal), 'convert'))
      .toEqual([...conversionsBeforeClear, ...regenerated])
    await site.writeContent('index.typ', 'Quiet image cache page.\n')
    await expect(page.locator('body')).toContainText('Quiet image cache page.')
    await expect.poll(async () => journalRecords(await readHookJournal(journal), 'reuse').length)
      .toBe(reusesBeforeClear + names.length)
    expect(journalRecords(await readHookJournal(journal), 'convert'))
      .toEqual([...conversionsBeforeClear, ...regenerated])
  } finally {
    holdConversions = false
    for (const response of held) response.end('continue')
    await gate.close()
  }
})

test('dev retains latest pending consumer', async ({ page, sites, directory }) => {
  const journal = join(directory, 'journal.txt')
  let firstResponse: ServerResponse | undefined
  const gate = await startGate((request, response) => {
    if (request.url === '/first') {
      firstResponse = response
    } else {
      response.end('continue')
    }
  })
  try {
    const site = await sites.dev({
      initialContent: { relativePath: 'index.typ', source: 'First consumer revision.\n' },
      beforeStart: async (root) => {
        const command = join(root, 'after-publish.mjs')
        await writeFile(
          command,
          `
          import { appendFile, readFile } from 'node:fs/promises';
          import { join } from 'node:path';
          const body = await readFile(join(process.env.TOLA_HOOK_INPUT_DIR, 'index.html'), 'utf8');
          const revision = body.includes('First consumer revision.') ? 'first'
            : body.includes('Second consumer revision.') ? 'second' : 'third';
          await appendFile(${JSON.stringify(journal)}, 'start:' + revision + '\\n');
          const response = await fetch(${JSON.stringify(gate.url)} + '/' + revision);
          await response.text();
          await appendFile(${JSON.stringify(journal)}, 'done:' + revision + '\\n');
        `,
        )
        await writeHookConfiguration(root, [
          hookToml('after-publish', command, { name: 'consumer', dev: 'run' }),
        ])
      },
    })
    await page.goto(site.url, { waitUntil: 'load' })
    await expect(page.locator('body')).toContainText('First consumer revision.')
    await expect.poll(() => readHookJournal(journal)).toBe('start:first\n')
    await expect.poll(() => firstResponse !== undefined).toBe(true)

    await site.writeContent('index.typ', 'Second consumer revision.\n')
    await expect(page.locator('body')).toContainText('Second consumer revision.')
    expect(await readHookJournal(journal)).toBe('start:first\n')

    await site.writeContent('index.typ', 'Third consumer revision.\n')
    await expect(page.locator('body')).toContainText('Third consumer revision.')
    expect(await readHookJournal(journal)).toBe('start:first\n')

    firstResponse!.end('continue')
    await expect.poll(() => readHookJournal(journal)).toBe(
      'start:first\ndone:first\nstart:third\ndone:third\n',
    )
  } finally {
    await gate.close()
  }
})

test('shutdown discards consumer work', async ({ page, sites, directory }) => {
  test.skip(
    process.platform === 'win32',
    'This shutdown contract needs a console interrupt; Node process pipes force-terminate Windows children.',
  )
  const journal = join(directory, 'shutdown-journal.txt')
  let started = false
  let closed = false
  const gate = await startGate((_request, response) => {
    started = true
    response.once('close', () => {
      closed = true
    })
  })
  try {
    const site = await sites.dev({
      initialContent: { relativePath: 'index.typ', source: 'Running consumer revision.\n' },
      beforeStart: async (root) => {
        const consumer = join(root, 'consume-published.mjs')
        await writeFile(
          consumer,
          `
          import { appendFile, readFile } from 'node:fs/promises';
          import { join } from 'node:path';
          const html = await readFile(join(process.env.TOLA_HOOK_INPUT_DIR, 'index.html'), 'utf8');
          const revision = html.includes('Running consumer revision.') ? 'running' : 'pending';
          await appendFile(${JSON.stringify(journal)}, 'start:' + revision + '\\n');
          const response = await fetch(${JSON.stringify(gate.url)});
          await response.text();
          await appendFile(${JSON.stringify(journal)}, 'done:' + revision + '\\n');
        `,
        )
        await writeHookConfiguration(root, [
          hookToml('after-publish', consumer, { name: 'consumer', dev: 'run' }),
        ])
      },
    })
    await page.goto(site.url, { waitUntil: 'load' })
    await expect.poll(() => started).toBe(true)
    await site.writeContent('index.typ', 'Pending consumer revision.\n')
    await expect(page.locator('body')).toContainText('Pending consumer revision.')
    await site.close()
    await expect.poll(() => closed).toBe(true)
    expect(await readHookJournal(journal)).toBe('start:running\n')
  } finally {
    await gate.close()
  }
})

test('consumer failure keeps published page', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'First committed page.\n' },
    beforeStart: async (root) => {
      const consumer = join(root, 'rejecting-consumer.mjs')
      await writeFile(
        consumer,
        `
        process.stderr.write('Consumer rejected revision');
        process.exit(7);
      `,
      )
      await writeHookConfiguration(root, [
        hookToml('after-publish', consumer, { name: 'consumer', dev: 'run' }),
      ])
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  // The browser indicator counts failures; the terminal names them.
  await expect.poll(() => site.stderr().match(/hook\.after_publish/g)?.length ?? 0).toBe(1)
  await expect(page.locator('body')).toContainText('First committed page.')
  await expect(page.locator('#tola-dev-status')).toBeVisible()
  const first = await currentRevision(page)

  await site.writeContent('index.typ', '#panic("Candidate rejected")\n')
  await expect.poll(site.stderr).toContain('Candidate rejected')
  await expect(page.locator('body')).toContainText('First committed page.')
  expect(await currentRevision(page)).toBe(first)

  await site.writeContent('index.typ', 'Second committed page.\n')
  await expect(page.locator('body')).toContainText('Second committed page.')
  await expect.poll(() => currentRevision(page)).not.toBe(first)
  await expect.poll(() => site.stderr().match(/hook\.after_publish/g)?.length ?? 0).toBe(2)
  await expect(page.locator('#tola-dev-status')).toBeVisible()
})

test('hook write of an unread file keeps the first build', async ({ page, sites }) => {
  let invocations!: string
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'First page.\n' },
    beforeStart: async (root) => {
      await writeSiteProgram(root, RENDER_EVERY_SOURCE)
      invocations = hookJournal(root)
      const command = join(root, 'write-unread.mjs')
      await writeFile(
        command,
        `
        import { appendFileSync, writeFileSync } from 'node:fs';
        appendFileSync(${JSON.stringify(invocations)}, 'run\\n');
        writeFileSync('content/extra.txt', 'Written by the source hook.\\n');
        writeFileSync('generated.txt', 'Declared output.\\n');
      `,
      )
      await writeUnreadContentFile(root)
      await writeHookConfiguration(root, [hookToml('before-build', command, {
        name: 'generator',
        outputs: ['generated.txt'],
      })])
    },
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(200)
  await expect(page.locator('body')).toContainText('First page.')
  expect(await readHookJournal(invocations)).toBe('run\n')

  await site.writeContent('index.typ', 'Second page.\n')
  await expect(page.locator('body')).toContainText('Second page.')
  await expect.poll(() => readHookJournal(invocations)).toBe('run\nrun\n')

  // A new source still rebuilds; the hook's own write adds no run of its own.
  const published = await currentRevision(page)
  await writeFile(join(site.root, 'content/added.typ'), 'Added source.\n')
  await expect.poll(() => currentRevision(page)).not.toBe(published)
  await expect.poll(() => readHookJournal(invocations)).toBe('run\nrun\nrun\n')
})

test('repair input reruns failed chain', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: '#read("../generated/c.txt")\n' },
    beforeStart: async (root) => {
      await mkdir(join(root, 'generated'))
      await writeFile(join(root, 'input.txt'), 'Source chain v1.')
      await writeFile(join(root, 'repair.txt'), 'blocked')
      const sourceA = join(root, 'source-a.mjs')
      const sourceB = join(root, 'source-b.mjs')
      const sourceC = join(root, 'source-c.mjs')
      await writeFile(
        sourceA,
        `
        import { readFileSync, writeFileSync } from 'node:fs';
        writeFileSync('generated/a.txt', readFileSync('input.txt'));
      `,
      )
      await writeFile(
        sourceB,
        `
        import { readFileSync } from 'node:fs';
        if (readFileSync('generated/a.txt', 'utf8') === 'Source chain v2.'
          && readFileSync('repair.txt', 'utf8') === 'blocked') {
          throw new Error('Middle source hook needs repair');
        }
      `,
      )
      await writeFile(
        sourceC,
        `
        import { readFileSync, writeFileSync } from 'node:fs';
        writeFileSync('generated/c.txt', readFileSync('generated/a.txt'));
      `,
      )
      await writeHookConfiguration(root, [
        hookToml('before-build', sourceA, {
          name: 'source-a',
          rerunOn: ['input.txt'],
          outputs: ['generated/a.txt'],
        }),
        hookToml('before-build', sourceB, { name: 'source-b', rerunOn: ['input.txt', 'repair.txt'] }),
        hookToml('before-build', sourceC, {
          name: 'source-c',
          rerunOn: ['generated/a.txt'],
          outputs: ['generated/c.txt'],
        }),
      ])
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Source chain v1.')

  await writeFile(join(site.root, 'input.txt'), 'Source chain v2.')
  await expect(page.locator('#tola-dev-status')).toBeVisible()
  await expect.poll(site.stderr).toContain('Middle source hook needs repair')
  expect(await readFile(join(site.root, 'generated/a.txt'), 'utf8')).toBe('Source chain v2.')
  expect(await readFile(join(site.root, 'generated/c.txt'), 'utf8')).toBe('Source chain v1.')
  await expect(page.locator('body')).toContainText('Source chain v1.')

  await writeFile(join(site.root, 'repair.txt'), 'ready')
  await expect(page.locator('body')).toContainText('Source chain v2.')
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()
  expect(await readFile(join(site.root, 'generated/c.txt'), 'utf8')).toBe('Source chain v2.')
})

test('failing generator stops rebuilding after one retry', async ({ sites }) => {
  let runs!: string
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'Page.\n' },
    beforeStart: async (root) => {
      runs = hookJournal(root, 'generator-runs.txt')
      const command = join(root, 'regenerate.mjs')
      await writeFile(
        command,
        `
        import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
        appendFileSync(${JSON.stringify(runs)}, 'run\\n');
        const count = readFileSync(${JSON.stringify(runs)}, 'utf8').trim().split('\\n').length;
        mkdirSync(\`generated/run-\${count}\`, { recursive: true });
        writeFileSync(\`generated/run-\${count}/mod.typ\`, '#let broken = (1 +\\n');
        writeFileSync('generated/manifest.typ', \`#import "run-\${count}/mod.typ": broken\\n\`);
      `,
      )
      await writeSiteProgram(
        root,
        '#import "generated/manifest.typ": broken\n#document("index.html")[Home]\n',
      )
      await writeHookConfiguration(root, [
        hookToml('before-build', command, { name: 'generator', outputs: ['generated/manifest.typ'] }),
      ])
    },
  })

  // The serving line follows the initial settle, so the failed chain has already run its one
  // retry: each failed attempt reports a round in the session log, and the journal holds one
  // generator run per attempt. A further attempt would take another input event.
  expect(site.stderr()).toContain('expected expression')
  await expect.poll(async () =>
    (await readSessionRecords(site.root)).filter((record) =>
      record.fields.kind === 'status' && typeof record.fields.round === 'number'
    ).length
  ).toBe(2)
  expect(await readHookJournal(runs)).toBe('run\nrun\n')
})
