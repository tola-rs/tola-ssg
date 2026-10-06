import { isDeepStrictEqual } from 'node:util'
import { isTomlTable, parseToml, tomlTable } from './cargo-toml.ts'
import { ReleaseError } from './release-error.ts'
import { versionKey } from './semver.ts'

/** Edit real string nodes, then compare every unrelated TOML value. */
export function updatedManifest(source: string, version: string, names: ReadonlySet<string>): string {
  versionKey(version)
  const document = parseToml(source)
  const workspace = tomlTable(document.values.workspace, 'workspace')
  const release = tomlTable(workspace.package, 'workspace.package')
  const changedPaths: string[][] = [['workspace', 'package', 'version']]
  release.version = version
  for (
    const [alias, dependency] of Object.entries(
      tomlTable(workspace.dependencies ?? {}, 'workspace.dependencies'),
    )
  ) {
    if (!isTomlTable(dependency)) continue
    const name = dependency.package ?? alias
    if (typeof name !== 'string' || !names.has(name)) continue
    dependency.version = version
    changedPaths.push(['workspace', 'dependencies', alias, 'version'])
  }
  const ranges = changedPaths
    .map((path) => {
      const literal = document.strings.find((entry) => isDeepStrictEqual(entry.path, path))
      if (!literal) throw new ReleaseError(`${path.join('.')} must define a literal string version`)
      return literal.range
    })
    .sort((left, right) => right[0] - left[0])
  const replacement = JSON.stringify(version)
  for (const [start, end] of ranges) source = source.slice(0, start) + replacement + source.slice(end)
  if (!isDeepStrictEqual(parseToml(source).values, document.values)) {
    throw new ReleaseError('version edit would change unrelated TOML values')
  }
  return source
}
