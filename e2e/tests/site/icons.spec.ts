import { expect } from '@playwright/test'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { test } from '../../support/server.ts'

/** Paints the left half from its own gradient and the right half from the instance's color. */
const MIXED_SVG =
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><defs><linearGradient id="paint"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><rect width="12" height="24" fill="url(#paint)"/><path d="M12 0H24V24H12Z" fill="currentColor"/></svg>`

const VIEWPORT_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><circle cx="20" cy="20" r="2" fill="red"/></svg>'

/** Two inline brand instances, one fixed-size viewport instance, and one published icon asset. */
const ICON_PAGE = `#import "@tola/icon:0.0.0": icon
#import "@tola/icon:0.0.0": icon-url
#for _ in range(2) {
  icon("brand:mark", label: "Brand", attrs: (class: "native-brand", style: "color: rgb(34, 197, 94)"))
}
#icon("brand:viewport", label: "Viewport", attrs: (id: "native-viewport", style: "font-size:16px"))
#html.img(id: "published-icon", src: icon-url("brand:viewport"), alt: "Published")
`

async function configureIconSite(root: string): Promise<void> {
  await mkdir(join(root, 'icons'))
  await writeFile(join(root, 'icons/mark.svg'), MIXED_SVG)
  await writeFile(join(root, 'icons/viewport.svg'), VIEWPORT_SVG)
  await writeFile(
    join(root, 'tola.toml'),
    `[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons"
[site]
base-path = "/docs/"
`,
  )
  const entry = join(root, 'site.typ')
  await writeFile(
    entry,
    '#import "@tola/icon:0.0.0": icon-bytes\n#asset("images/mark.svg", icon-bytes("brand:mark"))\n' +
      await readFile(entry, 'utf8'),
  )
}

test('inline icon instances paint independently', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: { relativePath: 'index.typ', source: ICON_PAGE },
    beforeStart: configureIconSite,
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(200)
  const brands = page.getByRole('img', { name: 'Brand', exact: true })
  await expect(brands).toHaveCount(2)
  const references = await brands.evaluateAll((elements) =>
    elements.map((element) => {
      const gradient = element.querySelector('linearGradient')!
      const rectangle = element.querySelector('rect')!
      return {
        id: gradient.id,
        fill: rectangle.getAttribute('fill'),
        color: getComputedStyle(element.querySelector('path')!).fill,
      }
    })
  )
  const [first, second] = references
  expect(first?.id).not.toBe(second?.id)
  for (const reference of references) {
    expect(reference.fill!.replaceAll('"', '')).toBe(`url(#${reference.id})`)
    expect(reference.color).toBe('rgb(34, 197, 94)')
  }
})

test('icon viewport sizes its artwork', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: { relativePath: 'index.typ', source: ICON_PAGE },
    beforeStart: configureIconSite,
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(200)
  const viewportGeometry = await page.locator('#native-viewport').evaluate((element) => {
    const viewport = element.getBoundingClientRect()
    const circle = element.querySelector('circle')!.getBoundingClientRect()
    return {
      width: viewport.width,
      height: viewport.height,
      circleFits: circle.left >= viewport.left && circle.right <= viewport.right &&
        circle.top >= viewport.top && circle.bottom <= viewport.bottom,
    }
  })
  expect(viewportGeometry.width).toBe(16)
  expect(viewportGeometry.height).toBe(16)
  expect(viewportGeometry.circleFits).toBe(true)
})

test('published icon assets serve their bytes', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: { relativePath: 'index.typ', source: ICON_PAGE },
    beforeStart: configureIconSite,
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(200)
  const asset = await page.request.get(new URL('/docs/images/mark.svg', site.url).href)
  expect(asset.ok()).toBeTruthy()
  expect(asset.headers()['content-type']).toContain('image/svg+xml')
  expect(await asset.text()).toContain('linearGradient id="paint"')
  const published = await page.locator('#published-icon').getAttribute('src')
  expect(published).toMatch(/^\/docs\/_tola\/icons\/[0-9a-f]{64}\.svg$/)
  const publishedAsset = await page.request.get(new URL(published!, site.url).href)
  expect(publishedAsset.ok()).toBeTruthy()
  expect(publishedAsset.headers()['content-type']).toContain('image/svg+xml')
  expect(await publishedAsset.text()).toContain('<circle')
})
