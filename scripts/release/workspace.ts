import { lstatSync, realpathSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { isDeepStrictEqual } from 'node:util'
import { type WorkspacePackage, workspacePackages } from './cargo-metadata.ts'
import { isTomlTable, readToml, tomlString, type TomlTable, tomlTable } from './cargo-toml.ts'
import { runChecked } from './command.ts'
import { ReleaseError } from './release-error.ts'
import { versionKey } from './semver.ts'

export interface CheckedWorkspace {
  readonly version: string
  readonly packages: readonly WorkspacePackage[]
}

export function workspaceVersion(root: string): string {
  const path = join(root, 'Cargo.toml')
  if (!lstatSync(path).isFile()) throw new ReleaseError(`workspace version requires a regular file: ${path}`)
  const workspace = tomlTable(readToml(path).workspace, 'Cargo.toml workspace')
  const value = tomlTable(workspace.package, 'Cargo.toml workspace.package').version
  if (typeof value !== 'string') {
    throw new ReleaseError('Cargo.toml must define [workspace.package].version as the single release version')
  }
  versionKey(value)
  return value
}

function* dependencyTables(manifest: TomlTable): Generator<readonly [string, TomlTable]> {
  const kinds = ['dependencies', 'dev-dependencies', 'build-dependencies'] as const
  for (const kind of kinds) yield [kind, tomlTable(manifest[kind] ?? {}, kind)]
  for (const [target, sections] of Object.entries(tomlTable(manifest.target ?? {}, 'target'))) {
    const tables = tomlTable(sections, `target.${target}`)
    for (const kind of kinds) {
      yield [`target.${target}.${kind}`, tomlTable(tables[kind] ?? {}, `target.${target}.${kind}`)]
    }
  }
}

export function checkManifests(root: string, packages: readonly WorkspacePackage[]): string {
  root = realpathSync(root)
  const version = workspaceVersion(root)
  const workspace = tomlTable(readToml(join(root, 'Cargo.toml')).workspace, 'workspace')
  const dependencies = tomlTable(workspace.dependencies ?? {}, 'workspace.dependencies')
  const members = new Map(packages.map((item) => [item.name, dirname(realpathSync(item.manifestPath))]))
  const memberDirectories = new Set(members.values())
  for (const [alias, dependency] of Object.entries(dependencies)) {
    if (!isTomlTable(dependency)) continue
    const name = tomlString(dependency.package ?? alias, `workspace.dependencies.${alias}.package`)
    const directory = dependency.path === undefined
      ? undefined
      : realpathSync(resolve(root, tomlString(dependency.path, `workspace.dependencies.${alias}.path`)))
    if (members.has(name) || (directory !== undefined && memberDirectories.has(directory))) {
      if (!members.has(name) || directory !== members.get(name) || dependency.version !== version) {
        throw new ReleaseError(
          `workspace dependency ${alias} must select its member path and version ${version}`,
        )
      }
      if ('git' in dependency || 'registry' in dependency) {
        throw new ReleaseError(
          `workspace dependency ${alias} must use the member's path and crates.io version`,
        )
      }
    }
  }
  for (const item of packages) {
    const manifest = readToml(item.manifestPath)
    const path = relative(root, item.manifestPath)
    const inheritedVersion = tomlTable(manifest.package, `${path}: package`).version
    if (
      !isTomlTable(inheritedVersion) ||
      Object.keys(inheritedVersion).length !== 1 ||
      inheritedVersion.workspace !== true
    ) {
      throw new ReleaseError(`${path}: package.version must inherit workspace.version`)
    }
    if (item.version !== version) {
      throw new ReleaseError(`${path}: effective version ${item.version} differs from ${version}`)
    }
    for (const [section, declarations] of dependencyTables(manifest)) {
      for (const [alias, declaration] of Object.entries(declarations)) {
        const inherited = isTomlTable(declaration) && declaration.workspace === true
        const dependency = inherited ? (dependencies[alias] ?? {}) : declaration
        const name = isTomlTable(dependency)
          ? tomlString(dependency.package ?? alias, `${section}.${alias}.package`)
          : alias
        const directory = isTomlTable(dependency) && dependency.path !== undefined
          ? realpathSync(
            resolve(
              inherited ? root : dirname(item.manifestPath),
              tomlString(dependency.path, `${section}.${alias}.path`),
            ),
          )
          : undefined
        if (members.has(name) || (directory !== undefined && memberDirectories.has(directory))) {
          if (!inherited) {
            throw new ReleaseError(`${path}: ${section}.${alias} must inherit its workspace dependency`)
          }
          if (
            !members.has(name) ||
            directory !== members.get(name) ||
            !isTomlTable(dependency) ||
            dependency.version !== version
          ) {
            throw new ReleaseError(
              `${path}: ${section}.${alias} does not select the synchronized workspace member`,
            )
          }
        }
      }
    }
  }
  return version
}

function lockPackages(source: TomlTable): TomlTable[] {
  if (!Array.isArray(source.package)) throw new ReleaseError('Cargo.lock package must be an array')
  return source.package.map((entry) => tomlTable(entry, 'Cargo.lock package'))
}

async function checkLock(
  root: string,
  packages: readonly WorkspacePackage[],
  signal?: AbortSignal,
): Promise<void> {
  await runChecked(root, ['cargo', 'metadata', '--locked', '--all-features', '--format-version', '1'], signal)
  const locked = lockPackages(readToml(join(root, 'Cargo.lock')))
  for (const item of packages) {
    const matching = locked.filter((entry) => entry.name === item.name && !('source' in entry))
    if (matching.length !== 1 || matching[0]?.version !== item.version) {
      throw new ReleaseError(`Cargo.lock does not contain workspace member ${item.name} ${item.version}`)
    }
  }
}

export async function checkVersion(root: string, signal?: AbortSignal): Promise<CheckedWorkspace> {
  const packages = await workspacePackages(root, signal)
  const version = checkManifests(root, packages)
  await checkLock(root, packages, signal)
  return { version, packages }
}

export function checkExternalResolution(original: TomlTable, replacement: TomlTable): void {
  const external = (source: TomlTable) =>
    lockPackages(source)
      .filter((entry) => 'source' in entry)
      .sort((left, right) => {
        for (const field of ['name', 'version', 'source'] as const) {
          const first = tomlString(left[field], `Cargo.lock ${field}`)
          const second = tomlString(right[field], `Cargo.lock ${field}`)
          if (first !== second) return first < second ? -1 : 1
        }
        return 0
      })
  if (!isDeepStrictEqual(external(original), external(replacement))) {
    throw new ReleaseError(
      'version update would change third-party dependency resolution; no files were applied',
    )
  }
}
