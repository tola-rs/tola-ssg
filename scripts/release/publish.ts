import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { parseArgs } from 'node:util'
import { CommandCancelled, withCancellation } from '../cancellation.ts'
import { REPOSITORY_ROOT } from '../paths.ts'
import { errorMessage } from '../process.ts'
import { checksumContents } from './checksums.ts'
import { checkTag, requireClean } from './checkout.ts'
import { checksums, licenseArchiveName } from './packaging.ts'
import { type PublishAsset, publishRelease } from './publish-release.ts'
import { checkSource } from './release-check.ts'
import { parseReleaseMode, type ReleaseMode } from './release-mode.ts'
import { TARGET_NAMES } from './targets.ts'

const HELP =
  `Usage: deno run -A scripts/release/publish.ts --tag TAG --commit SHA --repository OWNER/REPO --event {push,workflow_dispatch} --archives DIRECTORY [--mode MODE] [--notes FILE]
Verify the selected source and complete archive set before authenticated GitHub publication.
Requires GH_TOKEN. Modes:
  create (default): create a release; existing tags and published assets must already match.
  update-preserve-notes: replace builds and move the tag, retaining the release title and notes.
  update-regenerate-notes: replace builds, move the tag, and replace the title and notes.
Updates require workflow_dispatch and an existing mutable release. Only workflow_dispatch may
create a missing tag. --notes is required except in update-preserve-notes mode.
New releases remain drafts until every asset is verified; existing drafts remain unpublished.
`

type PublishCommand =
  | { readonly help: true }
  | {
    readonly help: false
    readonly tag: string
    readonly commit: string
    readonly repository: string
    readonly event: 'push' | 'workflow_dispatch'
    readonly mode: ReleaseMode
    readonly archives: string
    readonly notes: string
  }

function parseCommand(args: readonly string[]): PublishCommand {
  const { values } = parseArgs({
    args: [...args],
    options: {
      tag: { type: 'string' },
      commit: { type: 'string' },
      repository: { type: 'string' },
      event: { type: 'string' },
      mode: { type: 'string' },
      archives: { type: 'string' },
      notes: { type: 'string' },
      help: { type: 'boolean', short: 'h' },
    },
  })
  if (values.help) return { help: true }
  const { tag, commit, repository, event, archives, notes } = values
  const mode = parseReleaseMode(values.mode)
  if (
    tag === undefined ||
    commit === undefined ||
    repository === undefined ||
    archives === undefined ||
    (event !== 'push' && event !== 'workflow_dispatch')
  ) {
    throw new Error('--tag, --commit, --repository, --event, and --archives are required')
  }
  if (mode !== 'update-preserve-notes' && notes === undefined) {
    throw new Error('--notes is required for create and update-regenerate-notes')
  }
  return { help: false, tag, commit, repository, event, mode, archives, notes: notes ?? '' }
}

export async function main(args: readonly string[] = process.argv.slice(2)): Promise<number> {
  let command: PublishCommand
  try {
    command = parseCommand(args)
  } catch (error) {
    console.error(`${HELP}\npublish: ${errorMessage(error)}`)
    return 2
  }
  if (command.help) {
    console.log(HELP)
    return 0
  }
  try {
    return await withCancellation(async (signal) => {
      const token = process.env.GH_TOKEN
      if (!token) throw new Error('GH_TOKEN is required')
      const checked = await checkSource(REPOSITORY_ROOT, command.tag, command.commit, signal, command.mode)
      // This is the sole publication-boundary archive verification. Its returned
      // digests also authenticate remote reruns without rereading archive bytes.
      const digests = await checksums(command.archives, checked.version, TARGET_NAMES, signal)
      const manifest = checksumContents(digests)
      const assets: PublishAsset[] = await Promise.all(
        [...digests].map(async ([name, sha256]) => ({
          name,
          sha256,
          content: new Blob([await Deno.readFile(join(command.archives, name), { signal })]),
        })),
      )
      assets.push({
        name: 'checksums.txt',
        sha256: createHash('sha256').update(manifest, 'ascii').digest('hex'),
        content: new Blob([manifest], { type: 'text/plain' }),
      })
      const licenseArchive = licenseArchiveName(checked.version)
      const licenseContents = await Deno.readFile(join(command.archives, licenseArchive), { signal })
      assets.push({
        name: licenseArchive,
        sha256: createHash('sha256').update(licenseContents).digest('hex'),
        content: new Blob([licenseContents], { type: 'application/gzip' }),
      })
      const notes = command.mode === 'update-preserve-notes'
        ? ''
        : await readFile(command.notes, { encoding: 'utf8', signal })
      signal.throwIfAborted()
      requireClean(REPOSITORY_ROOT)
      checkTag(REPOSITORY_ROOT, command.tag, checked.commit, command.mode)
      const result = await publishRelease(
        {
          repository: command.repository,
          tag: command.tag,
          commit: checked.commit,
          event: command.event,
          mode: command.mode,
          notes,
          assets,
        },
        token,
        fetch,
        signal,
        process.env.GITHUB_API_URL,
      )
      switch (result.state) {
        case 'published':
          console.log(`Published ${command.tag} only after verifying every uploaded asset.`)
          break
        case 'unchanged':
          console.log(`Published release ${command.tag} already matches; no remote changes were made.`)
          break
        case 'updated':
          console.log(
            `Updated ${command.tag} to ${checked.commit}; title and notes ${
              command.mode === 'update-preserve-notes' ? 'preserved' : 'regenerated'
            }.`,
          )
          break
        case 'draft':
          console.log(
            `Retained existing draft ${command.tag} (release ${result.releaseId}) with complete assets. It is NOT published; review and publish it deliberately.`,
          )
          break
      }
      return 0
    })
  } catch (error) {
    console.error(`publish: ${errorMessage(error)}`)
    console.error(
      'Publication stopped. GitHub updates are not atomic; inspect the tag and assets before rerunning. No automatic rollback is attempted.',
    )
    return error instanceof CommandCancelled ? error.exitCode : 1
  }
}

if (import.meta.main) process.exitCode = await main()
