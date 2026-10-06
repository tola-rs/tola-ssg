import { lstatSync, realpathSync } from 'node:fs'
import { isAbsolute, relative, sep } from 'node:path'
import { runChecked } from './command.ts'
import { ReleaseError } from './release-error.ts'

export interface WorkspacePackage {
  readonly id: string
  readonly name: string
  readonly version: string
  readonly manifestPath: string
  readonly publish: readonly string[] | null
}

function object(value: unknown, description: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new ReleaseError(`${description} must be an object`)
  }
  return value as Record<string, unknown>
}

function string(value: unknown, description: string): string {
  if (typeof value !== 'string') throw new ReleaseError(`${description} must be a string`)
  return value
}

function strings(value: unknown, description: string): string[] {
  if (!Array.isArray(value)) throw new ReleaseError(`${description} must be an array`)
  return value.map((item: unknown) => string(item, description))
}

export async function workspacePackages(root: string, signal?: AbortSignal): Promise<WorkspacePackage[]> {
  const decoded: unknown = JSON.parse(
    await runChecked(root, ['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], signal),
  )
  const description = object(decoded, 'Cargo metadata')
  const workspaceRoot = realpathSync(root)
  if (realpathSync(string(description.workspace_root, 'Cargo workspace root')) !== workspaceRoot) {
    throw new ReleaseError('release commands must run at the Cargo workspace root')
  }
  const members = new Set(strings(description.workspace_members, 'Cargo workspace members'))
  if (!Array.isArray(description.packages)) throw new ReleaseError('Cargo packages must be an array')
  const packages: WorkspacePackage[] = []
  for (const raw of description.packages as unknown[]) {
    const item = object(raw, 'Cargo package')
    const id = string(item.id, 'Cargo package ID')
    if (!members.has(id)) continue
    const manifestPath = string(item.manifest_path, 'Cargo manifest path')
    const memberPath = relative(workspaceRoot, realpathSync(manifestPath))
    if (
      memberPath === '..' ||
      memberPath.startsWith(`..${sep}`) ||
      isAbsolute(memberPath) ||
      !lstatSync(manifestPath).isFile()
    ) {
      throw new ReleaseError(`workspace manifest must be a regular file inside the checkout: ${manifestPath}`)
    }
    packages.push({
      id,
      name: string(item.name, 'Cargo package name'),
      version: string(item.version, 'Cargo package version'),
      manifestPath,
      publish: item.publish === null || item.publish === undefined
        ? null
        : strings(item.publish, 'Cargo publication registries'),
    })
  }
  if (packages.length !== members.size) throw new ReleaseError('Cargo metadata omitted a workspace member')
  return packages.sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0))
}
