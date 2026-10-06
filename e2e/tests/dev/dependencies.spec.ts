import { expect } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { test } from '../../support/server.ts'

/** The manifest every `@local/demo:1.0.0` copy carries. */
const DEMO_MANIFEST =
  '[package]\nname = "demo"\nversion = "1.0.0"\nentrypoint = "lib.typ"\nauthors = ["Tola E2E"]\n'

/** Writes the demo package's manifest and `lib.typ`, creating `directory`. */
async function writeDemoPackage(directory: string, body: string): Promise<void> {
  await mkdir(directory, { recursive: true })
  await writeFile(join(directory, 'typst.toml'), DEMO_MANIFEST)
  await writeFile(join(directory, 'lib.typ'), body)
}

test('runtime dependency edit republishes page', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: null,
  })
  const template = join(site.root, 'site/layout.txt')
  await page.goto(site.url, { waitUntil: 'load' })
  await writeFile(template, 'Template version one.\n')
  const firstNavigation = page.waitForEvent('load')
  await site.writeContent(
    'index.typ',
    '#let name = "layout"\n#read("../site/" + name + ".txt")\n',
  )
  await firstNavigation
  await expect(page.locator('body')).toContainText('Template version one.')

  await writeFile(template, 'Template version two.\n')

  await expect(page.locator('body')).toContainText('Template version two.')
  await expect(page.locator('body')).not.toContainText('Template version one.')
})

test('created runtime dependency recovers page', async ({ page, sites }) => {
  const site = await sites.dev()
  const template = join(site.root, 'site/missing.txt')
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Hello from the process E2E.')

  await site.writeContent(
    'index.typ',
    '#let name = "missing"\n#read("../site/" + name + ".txt")\n',
  )
  await expect.poll(() => site.stderr(), { timeout: 5_000 }).toContain('file not found')
  await expect(page.locator('body')).toContainText('Hello from the process E2E.')

  await writeFile(template, 'Recovered dependency.\n')

  await expect(page.locator('body')).toContainText('Recovered dependency.')
})

test('data package overrides cached package', async ({ page, sites }) => {
  const site = await sites.dev({
    packageDataRoot: 'package-data',
    packageCacheRoot: 'package-cache',
    beforeStart: async (root) => {
      await writeDemoPackage(
        join(root, 'package-cache/local/demo/1.0.0'),
        '#let value = [Cache package]\n',
      )
      await mkdir(join(root, 'package-data'), { recursive: true })
    },
    initialContent: {
      relativePath: 'index.typ',
      source: '#import "@local/demo:1.0.0": value\n#value\n',
    },
  })
  const packageDirectory = join(site.root, 'package-data/local/demo/1.0.0')
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Cache package')

  await writeDemoPackage(packageDirectory, '#let value = [Data package]\n')
  await expect(page.locator('body')).toContainText('Data package')
  await expect(page.locator('body')).not.toContainText('Cache package')
})

test('vendored package overrides user package', async ({ page, sites }) => {
  const site = await sites.dev({
    packageDataRoot: 'package-data',
    packageCacheRoot: 'package-cache',
    beforeStart: async (root) => {
      await writeDemoPackage(join(root, 'package-data/local/demo/1.0.0'), '#let value = [User package]\n')
      await writeDemoPackage(
        join(root, 'vendor/typst-packages/local/demo/1.0.0'),
        '#let value = [Vendored package]\n',
      )
    },
    initialContent: {
      relativePath: 'index.typ',
      source: '#import "@local/demo:1.0.0": value\n#value\n',
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Vendored package')
  await expect(page.locator('body')).not.toContainText('User package')

  await writeFile(
    join(site.root, 'vendor/typst-packages/local/demo/1.0.0/lib.typ'),
    '#let value = [Edited package]\n',
  )

  await expect(page.locator('body')).toContainText('Edited package')
  await expect(page.locator('body')).not.toContainText('Vendored package')
})
