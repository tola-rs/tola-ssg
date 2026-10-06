import { expect } from '@playwright/test'
import { test } from '../../support/server.ts'
import { RENDER_EVERY_SOURCE, writeSiteProgram } from '../../support/site.ts'

for (
  const { label, contentRoot, text } of [
    { label: 'default content root', contentRoot: 'content', text: 'The first published page.\n' },
    { label: 'configured content root', contentRoot: 'pages', text: 'The configured-root page.\n' },
  ]
) {
  test(`${label} publishes the first page`, async ({ page, sites }) => {
    const site = await sites.dev({
      contentRoot,
      initialContent: null,
      beforeStart: (root) => writeSiteProgram(root, RENDER_EVERY_SOURCE),
    })
    const response = await page.goto(site.url, { waitUntil: 'load' })
    expect(response?.status()).toBe(200)

    const navigation = page.waitForEvent('load')
    await site.writeContent('index.typ', text)
    await navigation

    await expect(page.locator('body')).toContainText(text)
  })
}

test('removing last page restores welcome', async ({ page, sites }) => {
  const site = await sites.dev({ beforeStart: (root) => writeSiteProgram(root, RENDER_EVERY_SOURCE) })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Hello from the process E2E.')

  const navigation = page.waitForEvent('load')
  await site.removeContent('index.typ')
  await navigation

  await expect(page.locator('body')).not.toContainText('Hello from the process E2E.')
  expect((await page.request.get(site.url)).status()).toBe(200)
})
