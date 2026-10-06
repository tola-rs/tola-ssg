import { ReleaseError } from './release-error.ts'
import { tagVersion, versionKey } from './semver.ts'

export type PublishRequest = (url: string, options: RequestInit) => Promise<Response>

export interface PublishAsset {
  readonly name: string
  readonly content: Blob
  readonly sha256: string
}

export interface PublishSelection {
  readonly repository: string
  readonly tag: string
  readonly commit: string
  readonly event: 'push' | 'workflow_dispatch'
  readonly notes: string
  readonly assets: readonly PublishAsset[]
}

interface RemoteRelease {
  readonly id: number
  readonly tag_name: string
  readonly draft: boolean
  readonly prerelease: boolean
  readonly upload_url: string
}

interface RemoteAsset {
  readonly name: string
  readonly size: number
  readonly state: string
  readonly digest: string | null
}

interface GitObject {
  readonly type: string
  readonly sha: string
}

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new ReleaseError('GitHub returned an invalid object')
  }
  return value as Record<string, unknown>
}

function gitObject(value: unknown): GitObject {
  const object = record(value)
  if (typeof object.type !== 'string' || typeof object.sha !== 'string') {
    throw new ReleaseError('GitHub returned an invalid Git object')
  }
  return { type: object.type, sha: object.sha }
}

function remoteRelease(value: unknown): RemoteRelease {
  const release = record(value)
  if (
    typeof release.id !== 'number' ||
    !Number.isSafeInteger(release.id) ||
    release.id <= 0 ||
    typeof release.tag_name !== 'string' ||
    typeof release.draft !== 'boolean' ||
    typeof release.prerelease !== 'boolean' ||
    typeof release.upload_url !== 'string'
  ) {
    throw new ReleaseError('GitHub returned an invalid release')
  }
  return {
    id: release.id,
    tag_name: release.tag_name,
    draft: release.draft,
    prerelease: release.prerelease,
    upload_url: release.upload_url,
  }
}

function remoteAsset(value: unknown): RemoteAsset {
  const asset = record(value)
  if (
    typeof asset.name !== 'string' ||
    typeof asset.size !== 'number' ||
    !Number.isSafeInteger(asset.size) ||
    asset.size < 0 ||
    typeof asset.state !== 'string' ||
    !(asset.digest === null || typeof asset.digest === 'string')
  ) {
    throw new ReleaseError('GitHub returned an invalid release asset')
  }
  return { name: asset.name, size: asset.size, state: asset.state, digest: asset.digest }
}

/** Authenticated JSON and uploads share bounded, cancellable, non-redirecting requests. */
class GitHub {
  readonly path: string
  readonly origin: string

  constructor(
    repository: string,
    readonly token: string,
    readonly request: PublishRequest,
    readonly signal?: AbortSignal,
    api = 'https://api.github.com',
  ) {
    if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
      throw new ReleaseError(`invalid GitHub repository: ${repository}`)
    }
    if (repository.split('/').some((part) => part === '.' || part === '..')) {
      throw new ReleaseError(`invalid GitHub repository: ${repository}`)
    }
    const url = new URL(api)
    if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) {
      throw new ReleaseError('GitHub API URL must be an HTTPS URL without credentials, query, or fragment')
    }
    if (!token) throw new ReleaseError('GH_TOKEN is required')
    this.origin = url.origin
    this.path = `${api.replace(/\/$/, '')}/repos/${repository}`
  }

  json(url: string, method = 'GET', body?: object, missing = false): Promise<unknown> {
    return this.send(
      url,
      method,
      body === undefined ? undefined : JSON.stringify(body),
      'application/json',
      missing,
    )
  }

  async send(
    url: string,
    method: string,
    body: RequestInit['body'],
    contentType: string,
    missing = false,
  ): Promise<unknown> {
    this.signal?.throwIfAborted()
    const deadline = AbortSignal.timeout(10 * 60 * 1000)
    const response = await this.request(url, {
      method,
      headers: {
        Accept: 'application/vnd.github+json',
        Authorization: `Bearer ${this.token}`,
        'Content-Type': contentType,
        'X-GitHub-Api-Version': '2022-11-28',
        'User-Agent': 'tola-release',
      },
      ...(body === undefined ? {} : { body }),
      signal: this.signal === undefined ? deadline : AbortSignal.any([this.signal, deadline]),
      redirect: 'error',
    })
    // A hidden/inaccessible resource is not evidence that a write is authorized.
    // Only callers checking refs or the release-by-tag endpoint permit a real 404.
    if (!response.ok) {
      await response.body?.cancel()
      this.signal?.throwIfAborted()
      if (missing && response.status === 404) return null
      throw new ReleaseError(`GitHub ${method} ${new URL(url).pathname}: HTTP ${response.status}`)
    }
    const result: unknown = await response.json()
    this.signal?.throwIfAborted()
    if (result === null) throw new ReleaseError('GitHub returned an empty object instead of a resource')
    return result
  }

  async list(path: string): Promise<unknown[]> {
    const values: unknown[] = []
    for (let page = 1;; page++) {
      const result = await this.json(`${this.path}${path}?per_page=100&page=${page}`)
      if (!Array.isArray(result)) throw new ReleaseError('GitHub returned an invalid resource list')
      values.push(...result)
      if (result.length < 100) return values
    }
  }

  async release(tag: string): Promise<RemoteRelease | null> {
    const result = await this.json(
      `${this.path}/releases/tags/${encodeURIComponent(tag)}`,
      'GET',
      undefined,
      true,
    )
    if (result !== null) return remoteRelease(result)
    // The tag endpoint documents published releases only. Authenticated listings
    // also include drafts, including those left behind by interrupted uploads.
    const matches = (await this.list('/releases'))
      .map(remoteRelease)
      .filter((release) => release.tag_name === tag)
    if (matches.length > 1) {
      throw new ReleaseError(`Multiple releases exist for ${tag}; refusing to choose one`)
    }
    return matches[0] ?? null
  }

  uploadUrl(release: RemoteRelease, name: string): string {
    const url = new URL(release.upload_url.replace(/\{.*$/, ''))
    const trustedOrigin = url.origin === this.origin ||
      (this.origin === 'https://api.github.com' && url.origin === 'https://uploads.github.com')
    if (!trustedOrigin || url.username || url.password || url.hash) {
      throw new ReleaseError('GitHub returned an untrusted asset upload URL')
    }
    const expected = `${new URL(this.path).pathname}/releases/${release.id}/assets`
    if (url.pathname !== expected) throw new ReleaseError('GitHub returned an unexpected asset upload path')
    url.search = ''
    url.searchParams.set('name', name)
    return url.href
  }
}

async function checkedTag(api: GitHub, selection: PublishSelection, allowMissing = false): Promise<boolean> {
  const { tag, commit } = selection
  const branch = await api.json(
    `${api.path}/git/ref/heads/${encodeURIComponent(tag)}`,
    'GET',
    undefined,
    true,
  )
  if (branch !== null) throw new ReleaseError(`Release tag conflicts with an existing branch: ${tag}`)
  const value = await api.json(`${api.path}/git/ref/tags/${encodeURIComponent(tag)}`, 'GET', undefined, true)
  if (value === null) {
    if (allowMissing) return false
    throw new ReleaseError(`Release tag is missing: ${tag}`)
  }
  const reference = record(value)
  if (reference.ref !== `refs/tags/${tag}`) {
    throw new ReleaseError(`GitHub returned an unexpected ref for ${tag}`)
  }
  let object = gitObject(reference.object)
  const seen = new Set<string>()
  while (object.type === 'tag') {
    if (seen.has(object.sha)) throw new ReleaseError(`Cyclic annotated tag: ${tag}`)
    seen.add(object.sha)
    const annotated = record(await api.json(`${api.path}/git/tags/${encodeURIComponent(object.sha)}`))
    object = gitObject(annotated.object)
  }
  if (object.type !== 'commit' || object.sha !== commit) {
    throw new ReleaseError(
      `Remote ${tag} resolves to ${object.type} ${object.sha}, not checked commit ${commit}`,
    )
  }
  return true
}

function checkRelease(release: RemoteRelease, tag: string, prerelease: boolean): void {
  if (release.tag_name !== tag) {
    throw new ReleaseError(`GitHub returned a release for another tag: ${release.tag_name}`)
  }
  if (release.prerelease !== prerelease) {
    throw new ReleaseError(
      `Existing ${tag} prerelease state conflicts with its SemVer; reconcile it manually`,
    )
  }
}

function checkAsset(remote: RemoteAsset, local: PublishAsset): void {
  if (
    remote.name !== local.name ||
    remote.state !== 'uploaded' ||
    remote.size !== local.content.size ||
    remote.digest?.toLowerCase() !== `sha256:${local.sha256}`
  ) {
    throw new ReleaseError(
      `Existing asset ${local.name} differs or lacks a verified SHA-256; refusing replacement`,
    )
  }
}

function missingAssets(remote: readonly RemoteAsset[], local: readonly PublishAsset[]): PublishAsset[] {
  const byName = new Map<string, RemoteAsset>()
  for (const asset of remote) {
    if (byName.has(asset.name)) throw new ReleaseError(`Duplicate remote asset: ${asset.name}`)
    byName.set(asset.name, asset)
  }
  return local.filter((asset) => {
    const existing = byName.get(asset.name)
    if (existing === undefined) return true
    checkAsset(existing, asset)
    return false
  })
}

export interface PublishResult {
  readonly state: 'published' | 'draft' | 'unchanged'
  readonly releaseId: number
}

/** Publish only prepared, locally verified assets; never move refs or delete/replace remote content. */
export async function publishRelease(
  selection: PublishSelection,
  token: string,
  request: PublishRequest = fetch,
  signal?: AbortSignal,
  apiUrl?: string,
): Promise<PublishResult> {
  const { tag, commit, assets } = selection
  const prerelease = versionKey(tagVersion(tag)).prerelease.length > 0
  if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(commit)) {
    throw new ReleaseError('Publication requires the full checked commit SHA')
  }
  if (assets.length === 0 || new Set(assets.map((asset) => asset.name)).size !== assets.length) {
    throw new ReleaseError('Publication requires a nonempty, unique verified asset set')
  }
  for (const asset of assets) {
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(asset.name) || !/^[0-9a-f]{64}$/.test(asset.sha256)) {
      throw new ReleaseError(`Invalid verified asset: ${asset.name}`)
    }
  }
  const api = new GitHub(selection.repository, token, request, signal, apiUrl)
  // Unconditional access check: a missing ref never hides an inaccessible commit.
  const remoteCommit = record(await api.json(`${api.path}/git/commits/${commit}`))
  if (remoteCommit.sha !== commit) throw new ReleaseError('GitHub returned a different selected commit')
  const tagExists = await checkedTag(api, selection, selection.event === 'workflow_dispatch')
  let release = await api.release(tag)
  const created = release === null
  if (release !== null) checkRelease(release, tag, prerelease)
  const missing = release === null
    ? [...assets]
    : missingAssets((await api.list(`/releases/${release.id}/assets`)).map(remoteAsset), assets)
  if (release !== null && !release.draft) {
    if (!tagExists || missing.length > 0) {
      throw new ReleaseError(`Published ${tag} is incomplete; refusing to change an already public release`)
    }
    return { state: 'unchanged', releaseId: release.id }
  }
  // Validate upload destinations before adding any remote state for existing drafts.
  if (release !== null) { for (const asset of missing) api.uploadUrl(release, asset.name) }
  if (!tagExists) {
    // Recheck after release/asset discovery as well; a concurrent creator still
    // fails at createRef rather than authorizing an update or force operation.
    await checkedTag(api, selection, true)
    await api.json(`${api.path}/git/refs`, 'POST', { ref: `refs/tags/${tag}`, sha: commit })
  }
  if (release === null) {
    await checkedTag(api, selection)
    release = remoteRelease(
      await api.json(`${api.path}/releases`, 'POST', {
        tag_name: tag,
        target_commitish: commit,
        name: tag,
        body: selection.notes,
        draft: true,
        prerelease,
        make_latest: 'false',
      }),
    )
    checkRelease(release, tag, prerelease)
  }
  const releaseId = release.id
  const requireDraft = async (): Promise<RemoteRelease> => {
    const current = remoteRelease(await api.json(`${api.path}/releases/${releaseId}`))
    if (current.id !== releaseId) throw new ReleaseError('GitHub returned a different release')
    checkRelease(current, tag, prerelease)
    if (!current.draft) {
      throw new ReleaseError(`Release ${tag} is no longer a draft; refusing further changes`)
    }
    return current
  }
  for (const asset of missing) {
    const current = await requireDraft()
    await checkedTag(api, selection)
    const uploaded = remoteAsset(
      await api.send(api.uploadUrl(current, asset.name), 'POST', asset.content, 'application/octet-stream'),
    )
    checkAsset(uploaded, asset)
  }
  const complete = (await api.list(`/releases/${releaseId}/assets`)).map(remoteAsset)
  if (missingAssets(complete, assets).length > 0) {
    throw new ReleaseError(`Release ${tag} still lacks required assets`)
  }
  await requireDraft()
  if (!created) {
    // A preexisting draft may be intentional. Reruns can finish its missing
    // uploads, but never publish it or overwrite title, notes, or other assets.
    return { state: 'draft', releaseId }
  }
  if (complete.length !== assets.length) throw new ReleaseError(`New draft ${tag} has unexpected assets`)
  await checkedTag(api, selection)
  const published = remoteRelease(
    await api.json(`${api.path}/releases/${releaseId}`, 'PATCH', {
      draft: false,
      prerelease,
      make_latest: prerelease ? 'false' : 'legacy',
    }),
  )
  checkRelease(published, tag, prerelease)
  if (published.id !== releaseId || published.draft) throw new ReleaseError(`GitHub did not publish ${tag}`)
  return { state: 'published', releaseId }
}
