import { expect } from '@playwright/test'
import { readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { commandRunner, expectExited } from '../../support/process.ts'
import { test } from '../../support/server.ts'

const STYLED_PAGE = `#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Starter styles]))

= Starter styles

- Semantic list

\`\`\`rust
fn main() { let answer = 42; }
\`\`\`

#html.elem("div", attrs: (class: "p-4"))[Tailwind spacing]
`

async function initializeSite(binary: string, root: string, args: string[]): Promise<void> {
  await rm(root, { recursive: true })
  expectExited(await commandRunner(binary)(['init', root, '--pure', ...args], dirname(root)))
}

for (const preset of ['medium', 'rich']) {
  test(`generated ${preset} styles follow browser themes`, async ({ binary, page, sites }) => {
    const site = await sites.preview({
      initialContent: null,
      inputScope: '--pure',
      beforeStart: async (root) => {
        await initializeSite(binary, root, ['--preset', preset])
        await writeFile(join(root, 'content/index.typ'), STYLED_PAGE)
      },
    })
    await page.emulateMedia({ colorScheme: 'light' })
    expect((await page.goto(site.url, { waitUntil: 'load' }))?.status()).toBe(200)
    await expect(page.locator('pre')).toHaveCSS('overflow-x', 'auto')
    await expect(page.locator('li')).toHaveCSS('list-style-type', 'disc')
    const heading = await page.getByRole('heading', { name: 'Starter styles' }).evaluate((node) =>
      parseFloat(getComputedStyle(node).fontSize)
    )
    const body = await page.locator('body').evaluate((node) => parseFloat(getComputedStyle(node).fontSize))
    expect(heading).toBeGreaterThan(body)
    if (preset === 'rich') await expect(page.locator('.p-4')).toHaveCSS('padding', '16px')

    const colors = () =>
      page.evaluate(() => ({
        text: getComputedStyle(document.documentElement).color,
        background: getComputedStyle(document.documentElement).backgroundColor,
        code: Array.from(
          document.querySelectorAll('.tola-code span'),
          (span) => getComputedStyle(span).color,
        ),
      }))
    const light = await colors()
    expect(light.code.length).toBeGreaterThan(0)

    await page.emulateMedia({ colorScheme: 'dark' })
    const dark = await colors()
    expect(dark.text).not.toBe(light.text)
    expect(dark.background).not.toBe(light.background)
    expect(dark.code).not.toEqual(light.code)

    await page.locator('html').evaluate((node) => node.setAttribute('data-theme', 'light'))
    expect(await colors()).toEqual(light)
    await page.emulateMedia({ colorScheme: 'light' })
    await page.locator('html').evaluate((node) => node.setAttribute('data-theme', 'dark'))
    expect(await colors()).toEqual(dark)
  })
}

test('generated search hook publishes under the deployment path', async ({ binary, page, sites }) => {
  const failures: string[] = []
  page.on('pageerror', (error) => failures.push(error.message))
  const site = await sites.preview({
    initialContent: null,
    inputScope: '--pure',
    beforeStart: async (root) => {
      await initializeSite(binary, root, ['--features', 'starter-stylesheet,pagefind,deno-toolchain'])
      const path = join(root, 'tola.toml')
      const configuration = await readFile(path, 'utf8')
      const mounted = configuration.replace(/^#?\s*base-path = .*$/m, 'base-path = "/docs/"')
      expect(mounted).not.toBe(configuration)
      await writeFile(path, mounted)
    },
  })
  const landing = await page.goto(site.url, { waitUntil: 'load' })
  expect(landing?.status()).toBe(200)
  const asset = 'assets/pagefind-search/pagefind.js'
  const response = await page.goto(new URL(asset, site.url).href, { waitUntil: 'load' })
  expect(response?.status()).toBe(200)
  expect(new URL(response!.url()).pathname).toBe(`/docs/${asset}`)
  expect(failures).toEqual([])
})
