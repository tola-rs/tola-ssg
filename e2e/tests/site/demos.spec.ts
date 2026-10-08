import { expect } from '@playwright/test'
import { test } from '../../support/process.ts'
import { previewDemo } from '../../support/server.ts'

test('demo contents links reach their own headings', async ({ binary, directory, page }) => {
  const preview = await previewDemo(binary, 'toc', directory)
  try {
    for (
      const [path, labels] of [
        ['', ['Start', 'Detail']],
        ['other/', ['Outside the guide']],
      ] as const
    ) {
      const address = new URL(path, preview.url)
      const response = await page.goto(address.href, { waitUntil: 'load' })
      expect(response?.status()).toBe(200)
      const contents = page.getByRole('navigation', { name: 'On this page' })
      await expect(contents.getByRole('link')).toHaveText([...labels])
      const targets = await contents.getByRole('link').evaluateAll((links) =>
        links.map((link) => {
          const destination = new URL((link as HTMLAnchorElement).href)
          const target = document.getElementById(decodeURIComponent(destination.hash.slice(1)))
          const heading = target?.closest('h1,h2,h3,h4,h5,h6') ??
            target?.querySelector('h1,h2,h3,h4,h5,h6')
          return {
            origin: destination.origin,
            path: destination.pathname,
            exists: target !== null,
            heading: heading?.textContent?.trim(),
          }
        })
      )
      expect(targets.map((target) => target.heading)).toEqual([...labels])
      for (const target of targets) {
        expect(target.exists).toBe(true)
        expect(target.origin).toBe(address.origin)
        expect(target.path).toBe(address.pathname)
      }
      if (path === '') {
        await expect(page.getByRole('heading', { name: 'Deep detail', exact: true })).toBeVisible()
        await expect(page.getByRole('heading', { name: 'Aside', exact: true })).toBeVisible()
      }
    }
  } finally {
    await preview.close()
  }
})

test('demo backlinks list each linking document once', async ({ binary, directory, page }) => {
  const preview = await previewDemo(binary, 'backlinks', directory)
  try {
    await page.goto(preview.url, { waitUntil: 'load' })
    const external = page.locator('article').getByRole('link')
    await expect(external).toHaveCount(1)
    expect(await external.getAttribute('href')).toBe('https://example.test/demo/topic/')
    await page.getByRole('navigation', { name: 'All pages' })
      .getByRole('link', { name: 'Topic', exact: true }).click()
    const incoming = page.getByRole('complementary', { name: 'Incoming pages' })
    await expect(incoming.getByRole('link')).toHaveCount(2)
    const destinations = await incoming.getByRole('link').evaluateAll((links) =>
      links.map((link) => (link as HTMLAnchorElement).href)
    )
    const titles: string[] = []
    for (const address of destinations) {
      await page.goto(address, { waitUntil: 'load' })
      const title = await page.getByRole('heading', { level: 1 }).innerText()
      titles.push(title)
      await expect(page.getByRole('complementary', { name: 'Incoming pages' }).getByRole('link'))
        .toHaveCount(0)
      await expect(page.getByRole('complementary', { name: 'Incoming pages' }))
        .toContainText('No incoming body links.')
      if (title === 'Alpha') {
        const bodyLinks = await page.locator('article a').evaluateAll((links) =>
          links.map((link) => (link as HTMLAnchorElement).href)
        )
        expect(bodyLinks).toHaveLength(2)
        for (const link of bodyLinks) {
          await page.goto(link, { waitUntil: 'load' })
          await expect(page.getByRole('heading', { name: 'Topic', level: 1, exact: true }))
            .toBeVisible()
        }
      }
    }
    expect(titles.sort()).toEqual(['Alpha', 'Beta'])
  } finally {
    await preview.close()
  }
})

test('demo icons preserve their paint', async ({ binary, directory, page }) => {
  const preview = await previewDemo(binary, 'media', directory)
  try {
    await page.goto(preview.url, { waitUntil: 'load' })
    const leaf = page.getByRole('img', { name: 'Leaf', exact: true })
    const sun = page.getByRole('img', { name: 'Sun', exact: true })
    await expect(leaf.locator('path').first()).toHaveCSS('fill', 'rgb(35, 85, 165)')
    await expect(sun.locator('circle')).toHaveCSS('fill', 'rgb(215, 120, 33)')
    await leaf.evaluate((icon) => {
      const paragraph = icon.closest('p')!
      paragraph.style.color = 'rgb(18, 90, 60)'
    })
    await expect(leaf.locator('path').first()).toHaveCSS('fill', 'rgb(18, 90, 60)')
    await expect(sun.locator('circle')).toHaveCSS('fill', 'rgb(215, 120, 33)')
  } finally {
    await preview.close()
  }
})

test('demo images decode their published dimensions', async ({ binary, directory, page }) => {
  const preview = await previewDemo(binary, 'media', directory)
  try {
    await page.goto(preview.url, { waitUntil: 'load' })
    for (
      const [name, expected] of [
        ['Three colored bands, resized', [48, 24]],
        ['The separately published original', [128, 64]],
      ] as const
    ) {
      const image = page.getByRole('img', { name, exact: true })
      const decoded = await image.evaluate(async (element: HTMLImageElement) => {
        await element.decode()
        return {
          width: element.naturalWidth,
          height: element.naturalHeight,
          source: element.currentSrc,
        }
      })
      expect([decoded.width, decoded.height]).toEqual([...expected])
      expect(new URL(decoded.source).origin).toBe(new URL(preview.url).origin)
    }
    const published = page.getByRole('img', {
      name: 'Leaf published as a separate image',
      exact: true,
    })
    const dimensions = await published.evaluate(async (image: HTMLImageElement) => {
      await image.decode()
      return [image.naturalWidth, image.naturalHeight]
    })
    expect(dimensions.every((dimension) => dimension > 0)).toBe(true)
    const source = await published.getAttribute('src')
    const svg = await page.request.get(new URL(source!, preview.url).href)
    expect(svg.ok()).toBe(true)
    expect(svg.headers()['content-type']).toContain('image/svg+xml')
  } finally {
    await preview.close()
  }
})

test('demo feed bodies follow their selection', async ({ binary, directory, page }) => {
  const preview = await previewDemo(binary, 'feeds', directory)
  try {
    await page.goto(preview.url, { waitUntil: 'load' })
    const links = await page.getByRole('navigation', { name: 'Feed choices' }).getByRole('link')
      .evaluateAll((anchors) =>
        anchors.map((anchor) => ({
          name: anchor.textContent!.trim(),
          url: (anchor as HTMLAnchorElement).href,
        }))
      )
    expect(links.map((link) => link.name).sort()).toEqual([
      'Portable text',
      'Selected body',
      'Summary',
      'Whole body',
    ])
    const feeds = Object.fromEntries(
      await Promise.all(links.map(async (link) => {
        const response = await page.request.get(link.url)
        expect(response.ok(), link.name).toBe(true)
        expect(response.headers()['content-type']).toContain('application/rss+xml')
        return [link.name, await response.text()]
      })),
    )
    await page.route('https://example.test/**', (route) => route.abort())
    const bodies = await page.evaluate((feeds: Record<string, string>) => {
      const parser = new DOMParser()
      return Object.fromEntries(
        Object.entries(feeds).map(([name, xml]) => {
          const rss = parser.parseFromString(xml, 'application/xml')
          if (rss.querySelector('parsererror')) throw new Error(`${name} is not valid XML`)
          const entry = rss.querySelector('item')
          if (!entry) throw new Error(`${name} has no feed entry`)
          const summary = parser.parseFromString(
            entry.querySelector('description')?.textContent ?? '',
            'text/html',
          )
          const encoded = entry.getElementsByTagNameNS(
            'http://purl.org/rss/1.0/modules/content/',
            'encoded',
          ).item(0)
          const content = parser.parseFromString(encoded?.textContent ?? '', 'text/html')
          return [name, {
            summary: summary.body.textContent?.replace(/\s+/g, ' ').trim(),
            hasBody: encoded !== null,
            strong: content.querySelector('strong')?.textContent,
            links: [...content.querySelectorAll('a')].map((link) => link.getAttribute('href')),
            headings: [...content.querySelectorAll('h1,h2,h3,h4,h5,h6')]
              .map((heading) => heading.textContent?.trim()),
            navigation: [...content.querySelectorAll('nav')]
              .map((nav) => nav.getAttribute('aria-label')),
            sidebar: content.querySelector('aside')?.textContent,
            images: [...content.querySelectorAll('img')].map((image) => image.getAttribute('src')),
          }]
        }),
      )
    }, feeds)
    const {
      Summary: summary,
      'Portable text': portable,
      'Whole body': whole,
      'Selected body': selected,
    } = bodies
    if (!summary || !portable || !whole || !selected) {
      throw new Error('the preview did not return all four named feed bodies')
    }
    for (const body of [summary, portable, whole, selected]) {
      expect(body.summary).toBe('A short summary.')
    }
    expect(summary.hasBody).toBe(false)
    expect(portable.strong).toBe('body')
    expect(portable.links).toContain('https://typst.app/')
    expect(whole.navigation.sort()).toEqual(['Feed choices', 'Primary'])
    expect(whole.sidebar).toContain('Outside the article')
    expect(selected.headings).toContain('Inside the article')
    expect(selected.navigation).toEqual([])
    expect(selected.sidebar).toBeUndefined()
    expect(selected.images).toEqual(['https://example.test/demo/assets/stripes.png'])
  } finally {
    await preview.close()
  }
})
