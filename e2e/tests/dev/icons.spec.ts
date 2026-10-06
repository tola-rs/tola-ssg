import { expect } from '@playwright/test'
import { mkdir, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { test } from '../../support/server.ts'
import { currentRevision } from '../../support/reload.ts'
import { proxyEnvironment, startRefusingProxy } from '../../support/proxy.ts'

test('missing local icon recovers', async ({ page, sites }) => {
  const iconContent = (name: string) =>
    `#import "@tola/icon:0.0.0": icon
#icon("brand:${name}", label: "Local icon", attrs: (id: "native-icon",))
`
  const circle =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="currentColor"/></svg>'
  const rectangle =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" fill="currentColor"/></svg>'
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: iconContent('mark') },
    beforeStart: async (root) => {
      await mkdir(join(root, 'icons'))
      await writeFile(join(root, 'icons/mark.svg'), circle)
      await writeFile(
        join(root, 'tola.toml'),
        `[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons"
`,
      )
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('#native-icon circle')).toHaveCount(1)

  await writeFile(join(site.root, 'icons/mark.svg'), rectangle)
  await expect(page.locator('#native-icon rect')).toHaveCount(1)

  const publishedRevision = await currentRevision(page)
  await rm(join(site.root, 'icons/mark.svg'))
  await expect(page.locator('#tola-dev-status')).toBeVisible()
  await expect(page.locator('#native-icon rect')).toHaveCount(1)
  expect(await currentRevision(page)).toBe(publishedRevision)

  await writeFile(join(site.root, 'icons/mark.svg'), circle)
  await expect(page.locator('#native-icon circle')).toHaveCount(1)
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()

  await site.writeContent('index.typ', iconContent('added'))
  await expect(page.locator('#tola-dev-status')).toBeVisible()
  await writeFile(join(site.root, 'icons/added.svg'), rectangle)
  await expect(page.locator('#native-icon rect')).toHaveCount(1)
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()
})

for (const inputScope of ['--offline', '--pure'] as const) {
  test(`${inputScope} survives icon configuration reloads`, async ({ page, sites }) => {
    const proxy = await startRefusingProxy()
    const remote = '[icons.collections.lucide]\nsource-type = "remote-json"\npreset = "lucide"\n'
    try {
      const site = await sites.dev({
        inputScope,
        environment: proxyEnvironment(proxy.url),
        initialContent: {
          relativePath: 'index.typ',
          source: '#import "@tola/icon:0.0.0": icon\n#icon("lucide:circle", attrs: (id: "scoped-icon",))',
        },
        beforeStart: async (root) => {
          await mkdir(join(root, 'icons'))
          await writeFile(
            join(root, 'icons/circle.svg'),
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/></svg>',
          )
          await writeFile(join(root, 'tola.toml'), remote)
        },
      })
      expect((await page.goto(site.url, { waitUntil: 'load' }))?.status()).toBe(503)
      expect(proxy.requests).toEqual([])
      await writeFile(
        join(site.root, 'tola.toml'),
        '[icons.collections.lucide]\nsource-type = "local-svg-dir"\npath = "icons"\n',
      )
      await expect(page.locator('#scoped-icon circle')).toHaveCount(1)
      const publishedRevision = await currentRevision(page)
      await writeFile(join(site.root, 'tola.toml'), remote)
      await expect(page.locator('#tola-dev-status')).toBeVisible()
      expect(await currentRevision(page)).toBe(publishedRevision)
      expect(proxy.requests).toEqual([])
      await site.close()
    } finally {
      await proxy.close()
    }
  })
}
