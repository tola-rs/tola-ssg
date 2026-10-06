import { expect } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { currentRevision } from '../../support/reload.ts'
import { test } from '../../support/server.ts'
import { writeMinimalSite } from '../../support/site.ts'

async function writePreviewSite(root: string): Promise<void> {
  await writeMinimalSite(root, {
    program: '#document("page/index.html", format: "html")[#include "content/page.typ"]\n',
  })
}

/** The header an editor sends when it confirms a publication. */
const previewHeader = { 'X-Tola-Preview': '1' }

function previewEndpoint(siteUrl: string, parameters: [string, string][]): URL {
  const endpoint = new URL('/_tola/preview', siteUrl)
  endpoint.search = new URLSearchParams(parameters).toString()
  return endpoint
}

test('preview serves one frozen site', async ({ page, sites }) => {
  const site = await sites.preview()
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Hello from the process E2E.')
  await expect(page.locator('script[data-tola-runtime]')).toHaveCount(0)

  await site.writeContent('index.typ', 'The source changed after preview started.\n')
  await page.reload({ waitUntil: 'load' })

  await expect(page.locator('body')).toContainText('Hello from the process E2E.')
  await expect(page.locator('body')).not.toContainText('The source changed after preview started.')
})

test('directory route keeps relative images', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: null,
    beforeStart: async (root) => {
      await writeMinimalSite(root, {
        program: `
#document("index.html")[Home]
#document("guide/index.html")[#html.img(src: "cover.svg")]
#asset("guide/cover.svg", "<svg xmlns='http://www.w3.org/2000/svg' width='30' height='30'><rect width='30' height='30' fill='red'/></svg>")
`,
      })
    },
  })
  // Only the trailing slash names the directory index; without it `guide` names a file.
  const missing = await page.request.get(new URL('/guide', site.url).href)
  expect(missing.status()).toBe(404)
  await page.goto(new URL('/guide/', site.url).href, { waitUntil: 'load' })
  const image = await page.locator('img').evaluate((element: HTMLImageElement) => ({
    url: element.currentSrc,
    width: element.naturalWidth,
  }))
  expect(image).toEqual({ url: new URL('/guide/cover.svg', site.url).href, width: 30 })
})

test('unchanged publication satisfies preview', async ({ page, request, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'page.typ', source: 'Mounted publication.\n' },
    beforeStart: async (root) => {
      await writePreviewSite(root)
      await writeFile(join(root, 'tola.toml'), '[site]\nbase-path = "/docs/"\n')
    },
  })
  const source = pathToFileURL(join(site.root, 'content/page.typ')).href
  const endpoint = previewEndpoint(site.url, [['source', source]])
  const response = await request.post(endpoint.href, { headers: previewHeader })
  expect(response.status()).toBe(200)
  const publication = await response.json()
  expect(publication.routes).toEqual([{ output: 'page/index.html', route: '/docs/page/' }])
  await page.goto(new URL(publication.routes[0].route, site.url).href)
  await expect(page.locator('body')).toContainText('Mounted publication.')
  expect(await currentRevision(page)).toBe(publication.revision)
  const unchanged = await request.post(endpoint.href, { headers: previewHeader })
  expect(unchanged.status()).toBe(200)
  expect(await unchanged.json()).toEqual(publication)
})

test('a mounted read answers the installed revision', async ({ request, sites }) => {
  // Nothing watches this site, so a read reaches the revision the start-up build published.
  const site = await sites.dev({
    watch: false,
    initialContent: { relativePath: 'page.typ', source: 'Installed publication.\n' },
    beforeStart: async (root) => {
      await writePreviewSite(root)
      await writeFile(join(root, 'tola.toml'), '[site]\nbase-path = "/docs/"\n')
    },
  })
  const source = pathToFileURL(join(site.root, 'content/page.typ')).href
  const installed = await request.get(previewEndpoint(site.url, [['source', source]]).href, {
    headers: previewHeader,
  })
  expect(installed.status()).toBe(200)
  const publication = await installed.json()
  expect(publication.routes).toEqual([{ output: 'page/index.html', route: '/docs/page/' }])

  await site.writeContent('page.typ', 'Uninstalled replacement.\n')

  const read = await request.get(
    previewEndpoint(site.url, [['source', source], ['revision', publication.revision]]).href,
    { headers: previewHeader },
  )
  expect(read.status()).toBe(200)
  expect(await read.json()).toEqual(publication)
  const page = await request.get(new URL('/docs/page/', site.url).href)
  expect(await page.text()).toContain('Installed publication.')
})

test('failed fences retain the publication', async ({ request, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'page.typ', source: 'Last installed publication.\n' },
    beforeStart: (root) => writePreviewSite(root),
  })
  const source = pathToFileURL(join(site.root, 'content/page.typ')).href
  const endpoint = previewEndpoint(site.url, [['source', source]])
  const installed = await request.post(endpoint.href, { headers: previewHeader })
  expect(installed.status()).toBe(200)
  const previous = await installed.json()
  await site.writeContent('page.typ', '#panic("Rejected saved preview")\n')
  const rejected = await request.post(endpoint.href, { headers: previewHeader })
  expect(rejected.status()).toBe(409)
  expect(await rejected.json()).toMatchObject({ revision: previous.revision, routes: [] })
  const page = await request.get(new URL('/page/', site.url).href)
  expect(page.status()).toBe(200)
  expect(await page.text()).toContain('Last installed publication.')
})

test('published reads reject stale revisions', async ({ request, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'page.typ', source: 'Earlier publication.\n' },
    beforeStart: (root) => writePreviewSite(root),
  })
  const source = pathToFileURL(join(site.root, 'content/page.typ')).href
  const endpoint = previewEndpoint(site.url, [['source', source]])
  const installed = await request.post(endpoint.href, { headers: previewHeader })
  expect(installed.status()).toBe(200)
  const previous = await installed.json()
  await site.writeContent('page.typ', 'Replacement publication.\n')
  const replacement = await request.post(endpoint.href, { headers: previewHeader })
  expect(replacement.status()).toBe(200)
  const current = await replacement.json()
  expect(current.revision).not.toBe(previous.revision)
  const stale = await request.get(
    previewEndpoint(site.url, [['source', source], ['revision', previous.revision]]).href,
    { headers: previewHeader },
  )
  expect(stale.status()).toBe(409)
  const page = await request.get(new URL(current.routes[0].route, site.url).href)
  expect(page.status()).toBe(200)
  expect(await page.text()).toContain('Replacement publication.')
})

/** One preview request the endpoint must refuse, and what makes it refused. */
type PreviewRefusal = {
  /** Read after `preview refuses a request`. */
  readonly reason: string
  readonly status: number
  readonly headers: Record<string, string>
  readonly parameters: (source: string, root: string) => [string, string][]
}

const previewRefusals: readonly PreviewRefusal[] = [
  {
    reason: 'without the preview header',
    status: 403,
    headers: {},
    parameters: (source) => [['source', source]],
  },
  {
    reason: 'carrying an origin header',
    status: 403,
    headers: { ...previewHeader, Origin: 'https://foreign.example' },
    parameters: (source) => [['source', source]],
  },
  {
    reason: 'with a preview header that is not 1',
    status: 403,
    headers: { 'X-Tola-Preview': 'true' },
    parameters: (source) => [['source', source]],
  },
  {
    reason: 'with an unknown query parameter',
    status: 400,
    headers: previewHeader,
    parameters: (source) => [['source', source], ['bogus', '1']],
  },
  {
    reason: 'with a repeated source',
    status: 400,
    headers: previewHeader,
    parameters: (source) => [['source', source], ['source', source]],
  },
  {
    reason: 'with a source outside the site',
    status: 400,
    headers: previewHeader,
    parameters: (_source, root) => [['source', pathToFileURL(join(root, '..', 'outside.typ')).href]],
  },
  {
    reason: 'with a source that is not a file URI',
    status: 400,
    headers: previewHeader,
    parameters: () => [['source', 'https://example.com/page.typ']],
  },
  {
    reason: 'when no watcher is running',
    status: 503,
    headers: previewHeader,
    parameters: (source) => [['source', source]],
  },
]

for (const refusal of previewRefusals) {
  test(`preview refuses a request ${refusal.reason}`, async ({ request, sites }) => {
    // Nothing watches this site, so only a request that reached publication could install the
    // newer saved source as a revision.
    const site = await sites.dev({
      watch: false,
      initialContent: { relativePath: 'page.typ', source: 'Installed publication.\n' },
      beforeStart: (root) => writePreviewSite(root),
    })
    const source = pathToFileURL(join(site.root, 'content/page.typ')).href
    const confirmed = previewEndpoint(site.url, [['source', source]])
    const installed = await request.get(confirmed.href, { headers: previewHeader })
    expect(installed.status()).toBe(200)
    const publication = await installed.json()

    await site.writeContent('page.typ', 'Refused publication.\n')

    const refused = await request.post(
      previewEndpoint(site.url, refusal.parameters(source, site.root)).href,
      { headers: refusal.headers },
    )
    expect(refused.status()).toBe(refusal.status)

    const current = await request.get(confirmed.href, { headers: previewHeader })
    expect(current.status()).toBe(200)
    expect(await current.json()).toEqual(publication)
    const page = await request.get(new URL('/page/', site.url).href)
    expect(await page.text()).toContain('Installed publication.')
  })
}
