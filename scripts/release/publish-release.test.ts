import { expect } from '@std/expect'
import { test } from '@std/testing/bdd'
import { createHash } from 'node:crypto'
import { publishRelease, type PublishRequest, type PublishSelection } from './publish-release.ts'
import { ReleaseError } from './release-error.ts'

const COMMIT = 'a'.repeat(40)
const OTHER = 'b'.repeat(40)
const TAG = 'v0.8.0'
const ROOT = '/repos/owner/tola'

type GitObject = { type: string; sha: string }
type Asset = { name: string; size: number; state: string; digest: string | null }
type Release = {
  id: number
  tag_name: string
  draft: boolean
  prerelease: boolean
  upload_url: string
  name: string
  body: string
  make_latest: string
}

function selection(tag = TAG, event: PublishSelection['event'] = 'push'): PublishSelection {
  return {
    repository: 'owner/tola',
    tag,
    commit: COMMIT,
    event,
    notes: 'Checked release notes',
    assets: ['tola-linux.tar.gz', 'checksums.txt'].map((name) => ({
      name,
      content: new Blob([name]),
      sha256: createHash('sha256').update(name).digest('hex'),
    })),
  }
}

/** Stateful fake GitHub: only injected requests can reach it, never the network. */
class FakeGitHub {
  tag: GitObject | null = { type: 'commit', sha: COMMIT }
  branch = false
  ref = `refs/tags/${TAG}`
  readonly annotated = new Map<string, GitObject>()
  release: Release | null = null
  otherReleases: Release[] = []
  assets: Asset[] = []
  writes = 0
  readonly published: string[][] = []
  failure: { path: string; status: number } | undefined
  failUpload: string | undefined
  moveTagAfterUpload = false
  abortAfterUpload: AbortController | undefined

  snapshot(): string {
    return JSON.stringify({ tag: this.tag, branch: this.branch, release: this.release, assets: this.assets })
  }

  existing(input: PublishSelection, draft: boolean): void {
    this.ref = `refs/tags/${input.tag}`
    this.release = {
      id: 1,
      tag_name: input.tag,
      draft,
      prerelease: input.tag.includes('-'),
      upload_url: `https://uploads.github.com${ROOT}/releases/1/assets{?name,label}`,
      name: 'Maintainer title',
      body: 'Maintainer notes',
      make_latest: 'false',
    }
    this.assets = input.assets.map((asset) => ({
      name: asset.name,
      size: asset.content.size,
      state: 'uploaded',
      digest: `sha256:${asset.sha256}`,
    }))
  }

  readonly request: PublishRequest = async (input, options) => {
    const request = new Request(input, options)
    const url = new URL(request.url)
    const path = url.pathname
    const method = request.method
    if (method !== 'GET') this.writes++
    if (this.failure?.path === path) return new Response('denied', { status: this.failure.status })
    if (method === 'GET') {
      if (path === `${ROOT}/git/commits/${COMMIT}`) return Response.json({ sha: COMMIT })
      if (path.startsWith(`${ROOT}/git/ref/heads/`)) {
        return this.branch
          ? Response.json({ ref: this.ref.replace('/tags/', '/heads/') })
          : new Response('', { status: 404 })
      }
      if (path.startsWith(`${ROOT}/git/ref/tags/`)) {
        return this.tag === null
          ? new Response('', { status: 404 })
          : Response.json({ ref: this.ref, object: this.tag })
      }
      if (path.startsWith(`${ROOT}/git/tags/`)) {
        const object = this.annotated.get(path.slice(`${ROOT}/git/tags/`.length))
        return object === undefined ? new Response('', { status: 404 }) : Response.json({ object })
      }
      if (path.startsWith(`${ROOT}/releases/tags/`)) {
        return this.release === null || this.release.draft
          ? new Response('', { status: 404 })
          : Response.json(this.release)
      }
      if (path === `${ROOT}/releases`) {
        const releases = [...this.otherReleases, ...(this.release === null ? [] : [this.release])]
        const start = (Number(url.searchParams.get('page')) - 1) * 100
        return Response.json(releases.slice(start, start + 100))
      }
      if (path === `${ROOT}/releases/1`) return Response.json(this.release)
      if (path === `${ROOT}/releases/1/assets`) {
        const start = (Number(url.searchParams.get('page')) - 1) * 100
        return Response.json(this.assets.slice(start, start + 100))
      }
    }
    if (method === 'POST' && path === `${ROOT}/git/refs`) {
      if (this.tag !== null) return new Response('ref already exists', { status: 422 })
      const body = (await request.json()) as Record<string, unknown>
      this.tag = { type: 'commit', sha: String(body.sha) }
      this.ref = String(body.ref)
      return Response.json({ ref: this.ref, object: this.tag }, { status: 201 })
    }
    if (method === 'POST' && path === `${ROOT}/releases`) {
      if (this.release !== null) return new Response('release already exists', { status: 422 })
      const body = (await request.json()) as Record<string, unknown>
      this.release = {
        id: 1,
        tag_name: String(body.tag_name),
        draft: body.draft === true,
        prerelease: body.prerelease === true,
        upload_url: `https://uploads.github.com${ROOT}/releases/1/assets{?name,label}`,
        name: String(body.name),
        body: String(body.body),
        make_latest: String(body.make_latest),
      }
      if (!this.release.draft) this.published.push(this.assets.map((asset) => asset.name))
      return Response.json(this.release, { status: 201 })
    }
    if (method === 'POST' && path === `${ROOT}/releases/1/assets`) {
      const name = url.searchParams.get('name') ?? ''
      if (this.assets.some((asset) => asset.name === name)) return new Response('duplicate', { status: 422 })
      if (name === this.failUpload) {
        // GitHub documents a 502 leaving a starter asset. It must not be deleted
        // automatically or mistaken for a complete, publishable upload.
        this.assets.push({ name, size: 0, state: 'starter', digest: null })
        return new Response('upload failed', { status: 502 })
      }
      const bytes = await request.arrayBuffer()
      const asset = {
        name,
        size: bytes.byteLength,
        state: 'uploaded',
        digest: `sha256:${createHash('sha256').update(new Uint8Array(bytes)).digest('hex')}`,
      }
      this.assets.push(asset)
      if (this.moveTagAfterUpload) this.tag = { type: 'commit', sha: OTHER }
      this.abortAfterUpload?.abort(new Error('cancelled upload'))
      return Response.json(asset, { status: 201 })
    }
    if (method === 'PATCH' && path === `${ROOT}/releases/1` && this.release !== null) {
      const body = (await request.json()) as Record<string, unknown>
      this.release = { ...this.release, ...body }
      if (!this.release.draft) this.published.push(this.assets.map((asset) => asset.name).sort())
      return Response.json(this.release)
    }
    throw new Error(`Unexpected fake GitHub request: ${method} ${path}`)
  }
}

test('branch conflicts, moved refs, unexpected refs, tag cycles, and missing push tags refuse every write', async () => {
  const cases = [
    (api: FakeGitHub) => {
      api.branch = true
    },
    (api: FakeGitHub) => {
      api.tag = { type: 'commit', sha: OTHER }
    },
    (api: FakeGitHub) => {
      api.ref = `refs/tags/${TAG}-other`
    },
    (api: FakeGitHub) => {
      api.tag = { type: 'tree', sha: COMMIT }
    },
    (api: FakeGitHub) => {
      api.tag = { type: 'tag', sha: OTHER }
      api.annotated.set(OTHER, { type: 'tag', sha: OTHER })
    },
    (api: FakeGitHub) => {
      api.tag = null
    },
  ]
  for (const configure of cases) {
    const api = new FakeGitHub()
    configure(api)
    const before = api.snapshot()
    await expect(publishRelease(selection(), 'token', api.request)).rejects.toThrow(ReleaseError)
    expect(api.snapshot()).toBe(before)
    expect(api.writes).toBe(0)
  }
})

test('HTTP authorization and server failures are not treated as absent refs or releases', async () => {
  for (
    const failure of [
      { path: `${ROOT}/git/commits/${COMMIT}`, status: 404 },
      { path: `${ROOT}/git/ref/heads/${TAG}`, status: 401 },
      { path: `${ROOT}/git/ref/tags/${TAG}`, status: 403 },
      { path: `${ROOT}/releases/tags/${TAG}`, status: 500 },
      { path: `${ROOT}/releases`, status: 403 },
    ]
  ) {
    const api = new FakeGitHub()
    api.tag = null
    api.failure = failure
    const before = api.snapshot()
    await expect(publishRelease(selection(TAG, 'workflow_dispatch'), 'token', api.request)).rejects.toThrow(
      ReleaseError,
    )
    expect(api.snapshot()).toBe(before)
    expect(api.writes).toBe(0)
  }
})

test('denied manual tag creation cannot create a release', async () => {
  const api = new FakeGitHub()
  api.tag = null
  api.failure = { path: `${ROOT}/git/refs`, status: 403 }
  const before = api.snapshot()
  await expect(publishRelease(selection(TAG, 'workflow_dispatch'), 'token', api.request)).rejects.toThrow(
    ReleaseError,
  )
  expect(api.snapshot()).toBe(before)
  expect(api.published).toEqual([])
})

test('nested annotated tags are peeled and new releases become visible only with the complete asset set', async () => {
  const api = new FakeGitHub()
  api.tag = { type: 'tag', sha: OTHER }
  api.annotated.set(OTHER, { type: 'tag', sha: 'c'.repeat(40) })
  api.annotated.set('c'.repeat(40), { type: 'commit', sha: COMMIT })
  const input = selection()
  const result = await publishRelease(input, 'token', api.request)
  expect(result.state).toBe('published')
  expect(api.published).toEqual([input.assets.map((asset) => asset.name).sort()])
  expect(api.release?.draft).toBe(false)
  expect(api.release?.prerelease).toBe(false)
  expect(api.release?.make_latest).toBe('legacy')
  expect(api.tag).toEqual({ type: 'tag', sha: OTHER })
})

test('manual prereleases create only the checked tag and never become latest', async () => {
  const api = new FakeGitHub()
  api.tag = null
  const input = selection('v0.8.0-pre.1+build', 'workflow_dispatch')
  await publishRelease(input, 'token', api.request)
  expect(api.tag as GitObject | null).toEqual({ type: 'commit', sha: COMMIT })
  expect(api.ref).toBe(`refs/tags/${input.tag}`)
  expect(api.release?.prerelease).toBe(true)
  expect(api.release?.make_latest).toBe('false')
  expect(api.published).toEqual([input.assets.map((asset) => asset.name).sort()])
})

test('upload failure retains the new draft and all partial assets without exposing a release', async () => {
  const api = new FakeGitHub()
  api.failUpload = 'checksums.txt'
  await expect(publishRelease(selection(), 'token', api.request)).rejects.toThrow(ReleaseError)
  expect(api.release?.draft).toBe(true)
  expect(api.assets.map((asset) => [asset.name, asset.state])).toEqual([
    ['tola-linux.tar.gz', 'uploaded'],
    ['checksums.txt', 'starter'],
  ])
  expect(api.published).toEqual([])
})

test('a ref moved during uploads prevents further uploads and publication', async () => {
  const api = new FakeGitHub()
  api.moveTagAfterUpload = true
  await expect(publishRelease(selection(), 'token', api.request)).rejects.toThrow(ReleaseError)
  expect(api.assets.map((asset) => asset.name)).toEqual(['tola-linux.tar.gz'])
  expect(api.release?.draft).toBe(true)
  expect(api.published).toEqual([])
})

test('cancellation during upload leaves a draft instead of attempting publication or rollback', async () => {
  const api = new FakeGitHub()
  const controller = new AbortController()
  api.abortAfterUpload = controller
  await expect(publishRelease(selection(), 'token', api.request, controller.signal)).rejects.toThrow(
    'cancelled upload',
  )
  expect(api.assets.map((asset) => asset.name)).toEqual(['tola-linux.tar.gz'])
  expect(api.release?.draft).toBe(true)
  expect(api.published).toEqual([])
})

test('matching published releases are unchanged, including metadata and unrelated historical assets', async () => {
  const api = new FakeGitHub()
  const input = selection()
  api.existing(input, false)
  api.assets.push({ name: 'tola-x86_64-linux.tar.gz', size: 123, state: 'uploaded', digest: null })
  const before = api.snapshot()
  const result = await publishRelease(input, 'token', api.request)
  expect(result.state).toBe('unchanged')
  expect(api.snapshot()).toBe(before)
  expect(api.writes).toBe(0)
})

test('drafts on later listing pages receive only missing assets and remain unpublished with their own notes', async () => {
  const api = new FakeGitHub()
  const input = selection()
  api.existing(input, true)
  const existing = api.release
  if (existing === null) throw new Error('missing seeded release')
  api.otherReleases = Array.from({ length: 100 }, (_, index) => ({
    ...existing,
    id: index + 2,
    tag_name: `v0.7.${index}`,
  }))
  api.assets.pop()
  const first = api.assets[0]
  if (first === undefined) throw new Error('missing seeded asset')
  const kept = { ...first }
  const result = await publishRelease(input, 'token', api.request)
  expect(result.state).toBe('draft')
  expect(api.release).toEqual(existing)
  expect(api.assets[0]).toEqual(kept)
  expect(api.assets.map((asset) => asset.name).sort()).toEqual(input.assets.map((asset) => asset.name).sort())
  expect(api.published).toEqual([])
})

test('conflicting or unverifiable existing assets prevent even additive draft uploads', async () => {
  for (const digest of [null, `sha256:${'0'.repeat(64)}`]) {
    const api = new FakeGitHub()
    const input = selection()
    api.existing(input, true)
    api.assets.pop()
    const first = api.assets[0]
    if (first === undefined) throw new Error('missing seeded asset')
    first.digest = digest
    const before = api.snapshot()
    await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
    expect(api.snapshot()).toBe(before)
    expect(api.writes).toBe(0)
  }
})

test('published missing assets and inconsistent prerelease metadata require deliberate reconciliation', async () => {
  for (const change of ['missing-asset', 'prerelease'] as const) {
    const api = new FakeGitHub()
    const input = selection()
    api.existing(input, false)
    if (change === 'missing-asset') api.assets.pop()
    else if (api.release !== null) api.release.prerelease = true
    const before = api.snapshot()
    await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
    expect(api.snapshot()).toBe(before)
    expect(api.writes).toBe(0)
  }
})

test('untrusted upload locations cannot receive credentials or trigger even tag creation', async () => {
  const api = new FakeGitHub()
  const input = selection(TAG, 'workflow_dispatch')
  api.existing(input, true)
  api.tag = null
  api.assets.pop()
  if (api.release !== null) api.release.upload_url = 'https://attacker.invalid/assets{?name}'
  const before = api.snapshot()
  await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
  expect(api.snapshot()).toBe(before)
  expect(api.writes).toBe(0)
})
