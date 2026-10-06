import { expect } from '@playwright/test'
import { readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { commandRunner, expectExited } from '../../support/process.ts'
import { test } from '../../support/server.ts'

const STYLED_PAGE = `#import "@tola/source:0.0.0": tola-meta
#import "@tola/web:0.0.0": math-svg
#tola-meta((title: [Starter styles]))

= Starter styles

- Semantic list

Inline #math-svg($x$).

#math-svg($ x + y_2 $)

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
        math: Array.from(
          document.querySelectorAll('.tola-math-inline [fill], .tola-math-block [fill]'),
          (glyph) => getComputedStyle(glyph).fill,
        ),
      }))
    const light = await colors()
    expect(light.code.length).toBeGreaterThan(0)
    expect(light.math.length).toBeGreaterThan(0)
    for (const fill of light.math) expect(fill).toBe(light.text)

    await page.emulateMedia({ colorScheme: 'dark' })
    const dark = await colors()
    expect(dark.text).not.toBe(light.text)
    expect(dark.background).not.toBe(light.background)
    expect(dark.code).not.toEqual(light.code)
    for (const fill of dark.math) expect(fill).toBe(dark.text)

    await page.locator('html').evaluate((node) => node.setAttribute('data-theme', 'light'))
    expect(await colors()).toEqual(light)
    await page.emulateMedia({ colorScheme: 'light' })
    await page.locator('html').evaluate((node) => node.setAttribute('data-theme', 'dark'))
    expect(await colors()).toEqual(dark)
  })
}

test('generated search follows the deployment path', async ({ binary, page, sites }) => {
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
      await writeFile(
        join(root, 'content/index.typ'),
        '#import "@tola/source:0.0.0": tola-meta\n#tola-meta((title: [Orchid handbook]))\n= Orchid handbook\n\nOrchids thrive with patient care.\n',
      )
    },
  })
  const resources: string[] = []
  page.on('response', (response) => {
    if (response.url().includes('pagefind-search')) {
      resources.push(new URL(response.url()).pathname)
      if (!response.ok()) failures.push(`${response.status()} ${response.url()}`)
    }
  })
  expect((await page.goto(new URL('search/', site.url).href, { waitUntil: 'load' }))?.status()).toBe(200)
  await page.locator('.pagefind-search').getByRole('textbox').fill('Orchid')
  const result = page.locator('.pagefind-ui__result-link').filter({ hasText: 'Orchid handbook' })
  await expect(result).toBeVisible()
  const themeColors = () =>
    page.evaluate(() => {
      const input = document.querySelector('.pagefind-ui__search-input')!
      const link = document.querySelector('.pagefind-ui__result-link')!
      return {
        page: getComputedStyle(document.documentElement).backgroundColor,
        text: getComputedStyle(document.documentElement).color,
        input: getComputedStyle(input).backgroundColor,
        inputText: getComputedStyle(input).color,
        link: getComputedStyle(link).color,
      }
    })
  await page.emulateMedia({ colorScheme: 'light' })
  const light = await themeColors()
  expect(light.input).toBe(light.page)
  expect(light.inputText).toBe(light.text)
  await page.emulateMedia({ colorScheme: 'dark' })
  const dark = await themeColors()
  expect(dark.input).toBe(dark.page)
  expect(dark.inputText).toBe(dark.text)
  expect(dark.input).not.toBe(light.input)
  expect(dark.link).not.toBe(light.link)
  await page.locator('html').evaluate((node) => node.setAttribute('data-theme', 'light'))
  expect(await themeColors()).toEqual(light)
  await page.emulateMedia({ colorScheme: 'light' })
  await page.locator('html').evaluate((node) => node.setAttribute('data-theme', 'dark'))
  expect(await themeColors()).toEqual(dark)

  const href = await result.getAttribute('href')
  expect(new URL(href!, site.url).pathname).toBe('/docs/')
  expect(resources.length).toBeGreaterThan(2)
  expect(resources.every((path) => path.startsWith('/docs/assets/pagefind-search/'))).toBe(true)
  await result.click()
  await expect(page).toHaveURL(new URL('/docs/', site.url).href)
  await expect(page.getByRole('heading', { name: 'Orchid handbook' })).toBeVisible()
  expect(failures).toEqual([])
})
