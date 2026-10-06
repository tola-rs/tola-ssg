import { errorMessage } from '../process.ts'
import type { WorkspacePackage } from './cargo-metadata.ts'
import { ReleaseError } from './release-error.ts'
import { compareVersions, versionKey } from './semver.ts'

export type RegistryRequest = (url: string, options: RequestInit) => Promise<Response>
interface RegistryVersion {
  readonly name: string
  readonly vers: string
}

export function registryPath(name: string): string {
  if (!name || /[^A-Za-z0-9_-]/.test(name)) throw new ReleaseError(`invalid crates.io package name: ${name}`)
  name = name.toLowerCase()
  if (name.length === 1) return `1/${name}`
  if (name.length === 2) return `2/${name}`
  if (name.length === 3) return `3/${name[0]}/${name}`
  return `${name.slice(0, 2)}/${name.slice(2, 4)}/${name}`
}

function registryVersion(line: string, name: string): RegistryVersion {
  let value: unknown
  try {
    value = JSON.parse(line)
  } catch (error) {
    throw new ReleaseError(`crates.io returned invalid index JSON for ${name}`, { cause: error })
  }
  if (
    typeof value !== 'object' ||
    value === null ||
    !('name' in value) ||
    typeof value.name !== 'string' ||
    !('vers' in value) ||
    typeof value.vers !== 'string'
  ) {
    throw new ReleaseError(`crates.io returned an invalid index entry for ${name}`)
  }
  return { name: value.name, vers: value.vers }
}

export async function checkAvailable(
  packages: readonly WorkspacePackage[],
  version: string,
  request: RegistryRequest = fetch,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  const requested = versionKey(version)
  for (const item of packages) {
    signal?.throwIfAborted()
    if (item.publish?.length === 0) continue
    if (item.publish !== null && !item.publish.includes('crates-io')) {
      throw new ReleaseError(`${item.name} is not configured for crates.io publication`)
    }
    const name = item.name
    let entries: string
    try {
      const response = await request(`https://index.crates.io/${registryPath(name)}`, {
        headers: { 'User-Agent': `tola-release-check/${version}`, 'Cache-Control': 'no-cache' },
        signal: signal === undefined
          ? AbortSignal.timeout(30_000)
          : AbortSignal.any([signal, AbortSignal.timeout(30_000)]),
      })
      signal?.throwIfAborted()
      if (response.status === 404) {
        await response.body?.cancel()
        console.log(`crates.io: no published package named ${name}.`)
        continue
      }
      if (!response.ok) {
        await response.body?.cancel()
        throw new ReleaseError(`cannot check crates.io package ${name}: HTTP ${response.status}`)
      }
      entries = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(
        await response.arrayBuffer(),
      )
      signal?.throwIfAborted()
    } catch (error) {
      if (signal?.aborted) {
        if (error === signal.reason || (error instanceof Error && error.name === 'AbortError')) {
          signal.throwIfAborted()
        }
        throw error
      }
      if (error instanceof ReleaseError) throw error
      throw new ReleaseError(`cannot check crates.io package ${name}: ${errorMessage(error)}`, {
        cause: error,
      })
    }
    if (!entries.trim()) throw new ReleaseError(`crates.io returned an empty index entry for ${name}`)
    const lines = entries.split(/\r\n|[\n\r]/)
    if (lines.at(-1) === '') lines.pop()
    for (const line of lines) {
      const entry = registryVersion(line, name)
      if (entry.name.replaceAll('_', '-').toLowerCase() !== name.replaceAll('_', '-').toLowerCase()) {
        throw new ReleaseError(`crates.io returned a different package in the index entry for ${name}`)
      }
      if (compareVersions(versionKey(entry.vers), requested) === 0) {
        throw new ReleaseError(
          `${name} ${version} is already published (including yanked versions); it cannot be replaced`,
        )
      }
    }
    console.log(`crates.io: ${name} ${version} has no published version collision.`)
  }
}
