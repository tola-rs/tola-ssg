import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setImmediate } from 'node:timers/promises'
import { structuredPatch } from 'diff'
import { parseToml } from './cargo-toml.ts'
import { applyFiles, readReleaseFile, type ReleaseFiles, withUpdateLock } from './checked-files.ts'
import { copyCheckout, requireClean } from './checkout.ts'
import { run, runChecked } from './command.ts'
import { updatedManifest } from './manifest.ts'
import { ReleaseError } from './release-error.ts'
import { compareVersions, versionKey } from './semver.ts'
import { checkExternalResolution, checkVersion, workspaceVersion } from './workspace.ts'

export interface VersionUpdate {
  readonly originals: ReleaseFiles
  readonly replacements: ReleaseFiles
}

export async function prepareUpdate(
  root: string,
  version: string,
  signal?: AbortSignal,
): Promise<VersionUpdate> {
  signal?.throwIfAborted()
  const requested = versionKey(version)
  run(root, ['git', 'check-ref-format', `refs/tags/v${version}`])
  const originals = new Map(
    ['Cargo.toml', 'Cargo.lock'].map((name) => [name, readReleaseFile(join(root, name))]),
  )
  const current = workspaceVersion(root)
  if (compareVersions(requested, versionKey(current)) < 0) {
    throw new ReleaseError(`refusing version downgrade from ${current} to ${version}`)
  }
  const { packages } = await checkVersion(root, signal)
  if (version === current) return { originals, replacements: new Map() }
  const manifest = originals.get('Cargo.toml')
  const lock = originals.get('Cargo.lock')
  if (!manifest || !lock) throw new ReleaseError('version preparation requires Cargo.toml and Cargo.lock')
  const decode = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })
  const rewritten = updatedManifest(
    decode.decode(manifest),
    version,
    new Set(packages.map((item) => item.name)),
  )
  const staged = mkdtempSync(join(tmpdir(), 'tola-version-'))
  const replacements = new Map<string, Buffer>()
  try {
    await copyCheckout(root, staged, signal)
    writeFileSync(join(staged, 'Cargo.toml'), rewritten, 'utf8')
    // Cargo, not the release tool, owns lockfile resolution and serialization.
    await runChecked(
      staged,
      ['cargo', 'metadata', '--offline', '--all-features', '--format-version', '1'],
      signal,
    )
    const stagedLock = readReleaseFile(join(staged, 'Cargo.lock'))
    checkExternalResolution(
      parseToml(decode.decode(lock)).values,
      parseToml(decode.decode(stagedLock)).values,
    )
    await checkVersion(staged, signal)
    for (const [name, original] of originals) {
      const content = name === 'Cargo.lock' ? stagedLock : readReleaseFile(join(staged, name))
      if (!content.equals(original)) replacements.set(name, content)
    }
    await setImmediate()
    signal?.throwIfAborted()
  } finally {
    rmSync(staged, { recursive: true, force: true })
  }
  return { originals, replacements }
}

function unifiedRange(start: number, length: number): string {
  return length === 0 ? `${start - 1},0` : length === 1 ? String(start) : `${start},${length}`
}

function showUpdate(originals: ReleaseFiles, replacements: ReleaseFiles): void {
  if (!replacements.size) console.log('Workspace already uses the requested version; no changes.')
  const decode = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })
  for (const [relative, content] of replacements) {
    const original = originals.get(relative)
    if (!original) throw new ReleaseError(`replacement has no checked original: ${relative}`)
    const patch = structuredPatch(
      relative,
      relative,
      decode.decode(original),
      decode.decode(content),
      undefined,
      undefined,
      { context: 3 },
    )
    process.stdout.write(`--- ${relative}\n+++ ${relative}\n`)
    for (const hunk of patch.hunks) {
      process.stdout.write(
        `@@ -${unifiedRange(hunk.oldStart, hunk.oldLines)} +${
          unifiedRange(hunk.newStart, hunk.newLines)
        } @@\n${hunk.lines.join('\n')}\n`,
      )
    }
  }
}

export async function updateVersion(
  root: string,
  version: string,
  apply = false,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted()
  if (!apply) {
    const { originals, replacements } = await prepareUpdate(root, version, signal)
    showUpdate(originals, replacements)
    return
  }
  await withUpdateLock(root, async () => {
    requireClean(root)
    const head = run(root, ['git', 'rev-parse', '--verify', 'HEAD'])
    const { originals, replacements } = await prepareUpdate(root, version, signal)
    await setImmediate()
    signal?.throwIfAborted()
    requireClean(root)
    if (run(root, ['git', 'rev-parse', '--verify', 'HEAD']) !== head) {
      throw new ReleaseError('Git HEAD changed during version preparation')
    }
    await applyFiles(root, originals, replacements, undefined, signal)
    showUpdate(originals, replacements)
    console.log(`Workspace version ${version} prepared. No commit, tag, push, or publication was performed.`)
  })
}
