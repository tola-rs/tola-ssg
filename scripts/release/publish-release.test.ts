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
type Asset = { id: number; name: string; size: number; state: string; digest?: string | null }
type Release = {
  id: number
  tag_name: string
  draft: boolean
  prerelease: boolean
  immutable: boolean
  upload_url: string
  name: string
  body: string
  make_latest: string
  discussion_url: string
}

function selection(
  tag = TAG,
  event: PublishSelection['event'] = 'push',
  mode: PublishSelection['mode'] = 'create',
): PublishSelection {
  return {
    repository: 'owner/tola',
    tag,
    commit: COMMIT,
    event,
    mode,
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
  readonly mutations: { method: string; path: string; body: unknown }[] = []
  readonly published: string[][] = []
  failure: { path: string; status: number } | undefined
  failUpload: string | undefined
  moveTagAfterUpload = false
  abortAfterUpload: AbortController | undefined
  private nextAssetId = 100

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
      immutable: false,
      upload_url: `https://uploads.github.com${ROOT}/releases/1/assets{?name,label}`,
      name: 'Maintainer title',
      body: 'Maintainer notes',
      make_latest: 'false',
      discussion_url: 'https://github.com/owner/tola/discussions/7',
    }
    this.assets = input.assets.map((asset) => ({
      id: this.nextAssetId++,
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
    if (method !== 'GET') {
      this.writes++
      this.mutations.push({
        method,
        path,
        body: request.headers.get('content-type') === 'application/json' && options.body !== undefined
          ? await request.clone().json()
          : null,
      })
    }
    if (this.failure?.path === path) return new Response('denied', { status: this.failure.status })
    if (method === 'GET') {
      if (path === `${ROOT}/git/commits/${COMMIT}`) return Response.json({ sha: COMMIT })
      if (path === `${ROOT}/git/commits/${OTHER}`) return Response.json({ sha: OTHER })
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
    if (method === 'PATCH' && path === `${ROOT}/git/refs/tags/${TAG}`) {
      const body = (await request.json()) as Record<string, unknown>
      if (body.force !== true) throw new Error('tag updates must force amended histories')
      this.tag = { type: 'commit', sha: String(body.sha) }
      return Response.json({ ref: this.ref, object: this.tag })
    }
    if (method === 'DELETE' && path.startsWith(`${ROOT}/releases/assets/`)) {
      const id = Number(path.slice(`${ROOT}/releases/assets/`.length))
      const index = this.assets.findIndex((asset) => asset.id === id)
      if (index < 0) return new Response('', { status: 404 })
      this.assets.splice(index, 1)
      return new Response(null, { status: 204 })
    }
    if (method === 'POST' && path === `${ROOT}/releases`) {
      if (this.release !== null) return new Response('release already exists', { status: 422 })
      const body = (await request.json()) as Record<string, unknown>
      this.release = {
        id: 1,
        tag_name: String(body.tag_name),
        draft: body.draft === true,
        prerelease: body.prerelease === true,
        immutable: false,
        upload_url: `https://uploads.github.com${ROOT}/releases/1/assets{?name,label}`,
        name: String(body.name),
        body: String(body.body),
        make_latest: String(body.make_latest),
        discussion_url: '',
      }
      if (!this.release.draft) this.published.push(this.assets.map((asset) => asset.name))
      return Response.json(this.release, { status: 201 })
    }
    if (method === 'POST' && path === `${ROOT}/releases/1/assets`) {
      const name = url.searchParams.get('name') ?? ''
      if (this.assets.some((asset) => asset.name === name)) return new Response('duplicate', { status: 422 })
      if (name === this.failUpload) {
        // GitHub can leave a starter asset after an upload fails with HTTP 502.
        this.assets.push({ id: this.nextAssetId++, name, size: 0, state: 'starter', digest: null })
        return new Response('upload failed', { status: 502 })
      }
      const bytes = await request.arrayBuffer()
      const asset = {
        id: this.nextAssetId++,
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
  api.assets.push({ id: 50, name: 'tola-x86_64-linux.tar.gz', size: 123, state: 'uploaded', digest: null })
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

test('updates replace owned assets while preserving release metadata', async () => {
  for (const draft of [false, true]) {
    const api = new FakeGitHub()
    const input = {
      ...selection(TAG, 'workflow_dispatch', 'update-preserve-notes'),
      commit: OTHER,
      notes: '',
    }
    api.existing(input, draft)
    if (api.release === null) throw new Error('missing seeded release')
    api.release.prerelease = true
    const originalRelease = { ...api.release }
    const equal = api.assets[0]
    const replaced = api.assets[1]
    if (equal === undefined || replaced === undefined) throw new Error('missing seeded assets')
    replaced.digest = null
    const extra = { id: 50, name: 'maintainer.zip', size: 123, state: 'uploaded', digest: null }
    api.assets.push(extra)
    const result = await publishRelease(input, 'token', api.request)
    expect(result).toEqual({ state: 'updated', releaseId: originalRelease.id })
    expect(api.release).toEqual(originalRelease)
    expect(api.tag).toEqual({ type: 'commit', sha: OTHER })
    expect(api.assets.find((asset) => asset.name === equal.name)).toEqual(equal)
    expect(api.assets.find((asset) => asset.name === extra.name)).toEqual(extra)
    expect(api.assets.find((asset) => asset.name === replaced.name)?.id).not.toBe(replaced.id)
    expect(api.mutations.map(({ method, path }) => [method, path])).toEqual([
      ['DELETE', `${ROOT}/releases/assets/${replaced.id}`],
      ['POST', `${ROOT}/releases/1/assets`],
      ['PATCH', `${ROOT}/git/refs/tags/${TAG}`],
    ])
    expect(api.mutations.at(-1)?.body).toEqual({ sha: OTHER, force: true })
  }
})

test('regenerated notes update only the existing title and body', async () => {
  for (const draft of [false, true]) {
    const api = new FakeGitHub()
    const input = { ...selection(TAG, 'workflow_dispatch', 'update-regenerate-notes'), commit: OTHER }
    api.existing(input, draft)
    const original = api.release
    const assets = [...api.assets]
    const result = await publishRelease(input, 'token', api.request)
    expect(result).toEqual({ state: 'updated', releaseId: 1 })
    expect(api.release).toEqual({ ...original, name: TAG, body: input.notes })
    expect(api.assets).toEqual(assets)
    expect(api.mutations.filter(({ path }) => path === `${ROOT}/releases/1`)).toEqual([
      { method: 'PATCH', path: `${ROOT}/releases/1`, body: { name: TAG, body: input.notes } },
    ])
    expect(api.tag).toEqual({ type: 'commit', sha: OTHER })
  }
})

test('updates repair mismatched, missing, and starter assets', async () => {
  for (const change of ['missing', 'digest', 'size', 'absent-digest', 'starter'] as const) {
    const api = new FakeGitHub()
    const input = selection(TAG, 'workflow_dispatch', 'update-preserve-notes')
    api.existing(input, false)
    const remote = api.assets[1]
    if (remote === undefined) throw new Error('missing seeded asset')
    if (change === 'missing') api.assets.pop()
    else if (change === 'digest') remote.digest = `sha256:${'0'.repeat(64)}`
    else if (change === 'size') remote.size = 0
    else if (change === 'absent-digest') delete remote.digest
    else remote.state = 'starter'
    await publishRelease(input, 'token', api.request)
    const local = input.assets[1]
    const uploaded = api.assets.find((asset) => asset.name === remote.name)
    if (local === undefined) throw new Error('missing selected asset')
    expect(uploaded?.size).toBe(local.content.size)
    expect(uploaded?.state).toBe('uploaded')
    expect(uploaded?.digest).toBe(`sha256:${local.sha256}`)
    expect(api.mutations.filter(({ method }) => method === 'DELETE').length).toBe(
      change === 'missing' ? 0 : 1,
    )
    expect(api.mutations.some(({ path }) => path.startsWith(`${ROOT}/git/refs`))).toBe(false)
  }
})

test('updates require existing mutable releases', async () => {
  for (const state of ['missing', 'immutable'] as const) {
    const api = new FakeGitHub()
    const input = { ...selection(TAG, 'workflow_dispatch', 'update-regenerate-notes'), commit: OTHER }
    if (state === 'immutable') {
      api.existing(input, false)
      if (api.release !== null) api.release.immutable = true
    }
    const before = api.snapshot()
    await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
    expect(api.snapshot()).toBe(before)
    expect(api.writes).toBe(0)
  }
})

test('update guards reject invalid publication selections', async () => {
  const input = selection(TAG, 'workflow_dispatch', 'update-preserve-notes')
  const cases: PublishSelection[] = [
    { ...input, event: 'push' },
    { ...input, mode: 'unknown' as PublishSelection['mode'] },
    { ...input, commit: 'abc' },
  ]
  for (const selected of cases) {
    await expect(publishRelease(selected, 'token', () => {
      throw new Error('invalid selections cannot reach GitHub')
    })).rejects.toThrow(ReleaseError)
  }
})

test('update tag validation refuses unsafe references', async () => {
  const input = { ...selection(TAG, 'workflow_dispatch', 'update-preserve-notes'), commit: OTHER }
  for (const change of ['branch', 'identity', 'tree', 'sha', 'cycle', 'missing', 'denied'] as const) {
    const api = new FakeGitHub()
    api.existing(input, false)
    if (change === 'branch') api.branch = true
    else if (change === 'identity') api.ref = `refs/tags/${TAG}-other`
    else if (change === 'tree') api.tag = { type: 'tree', sha: COMMIT }
    else if (change === 'sha') api.tag = { type: 'commit', sha: 'abc' }
    else if (change === 'cycle') {
      api.tag = { type: 'tag', sha: COMMIT }
      api.annotated.set(COMMIT, { type: 'tag', sha: COMMIT })
    } else if (change === 'missing') api.tag = null
    else api.failure = { path: `${ROOT}/git/commits/${OTHER}`, status: 404 }
    const before = api.snapshot()
    await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
    expect(api.snapshot()).toBe(before)
    expect(api.writes).toBe(0)
  }
})

test('updates replace annotated refs with the selected commit', async () => {
  const api = new FakeGitHub()
  const input = { ...selection(TAG, 'workflow_dispatch', 'update-preserve-notes'), commit: OTHER }
  api.existing(input, false)
  api.tag = { type: 'tag', sha: 'c'.repeat(40) }
  api.annotated.set('c'.repeat(40), { type: 'tag', sha: 'd'.repeat(40) })
  api.annotated.set('d'.repeat(40), { type: 'commit', sha: COMMIT })
  await publishRelease(input, 'token', api.request)
  expect(api.tag).toEqual({ type: 'commit', sha: OTHER })
  expect(api.mutations).toEqual([
    { method: 'PATCH', path: `${ROOT}/git/refs/tags/${TAG}`, body: { sha: OTHER, force: true } },
  ])
})

test('denied forced refs retain the previous tag and notes', async () => {
  const api = new FakeGitHub()
  const input = { ...selection(TAG, 'workflow_dispatch', 'update-regenerate-notes'), commit: OTHER }
  api.existing(input, false)
  api.failure = { path: `${ROOT}/git/refs/tags/${TAG}`, status: 403 }
  const before = api.snapshot()
  await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
  expect(api.snapshot()).toBe(before)
  expect(api.mutations).toEqual([
    { method: 'PATCH', path: `${ROOT}/git/refs/tags/${TAG}`, body: { sha: OTHER, force: true } },
  ])
})

test('failed update uploads retain partial assets before tag movement', async () => {
  const api = new FakeGitHub()
  const input = { ...selection(TAG, 'workflow_dispatch', 'update-regenerate-notes'), commit: OTHER }
  api.existing(input, false)
  const original = api.release
  for (const asset of api.assets) asset.digest = null
  api.failUpload = 'checksums.txt'
  await expect(publishRelease(input, 'token', api.request)).rejects.toThrow(ReleaseError)
  expect(api.release).toEqual(original)
  expect(api.tag).toEqual({ type: 'commit', sha: COMMIT })
  expect(api.assets.map(({ name, state }) => [name, state])).toEqual([
    ['tola-linux.tar.gz', 'uploaded'],
    ['checksums.txt', 'starter'],
  ])
  expect(api.mutations.some(({ method }) => method === 'PATCH')).toBe(false)
})

test('interrupted updates stop before tag movement or notes', async () => {
  for (const interruption of ['cancel', 'move'] as const) {
    const api = new FakeGitHub()
    const input = { ...selection(TAG, 'workflow_dispatch', 'update-regenerate-notes'), commit: OTHER }
    api.existing(input, false)
    const original = api.release
    api.assets = []
    const controller = new AbortController()
    if (interruption === 'cancel') api.abortAfterUpload = controller
    else api.moveTagAfterUpload = true
    await expect(publishRelease(input, 'token', api.request, controller.signal)).rejects.toThrow()
    expect(api.release).toEqual(original)
    expect(api.assets.map(({ name }) => name)).toEqual(['tola-linux.tar.gz'])
    expect(api.mutations.some(({ method }) => method === 'PATCH')).toBe(false)
    expect(api.tag).toEqual({ type: 'commit', sha: interruption === 'cancel' ? COMMIT : OTHER })
  }
})
