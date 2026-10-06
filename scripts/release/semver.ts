import { ReleaseError } from './release-error.ts'

const VERSION =
  /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/
const U64_MAX = (1n << 64n) - 1n

export interface SemVer {
  readonly core: readonly [bigint, bigint, bigint]
  readonly prerelease: readonly (bigint | string)[]
}

/** Build metadata does not participate in SemVer precedence. */
export function versionKey(value: string): SemVer {
  const match = VERSION.exec(value)
  if (
    !match ||
    match[0] !== value ||
    match[1] === undefined ||
    match[2] === undefined ||
    match[3] === undefined
  ) {
    throw new ReleaseError(`invalid SemVer version: ${JSON.stringify(value)}`)
  }
  const core = [BigInt(match[1]), BigInt(match[2]), BigInt(match[3])] as const
  if (core.some((part) => part > U64_MAX)) {
    throw new ReleaseError(`version component exceeds Cargo's u64 limit: ${value}`)
  }
  const prerelease = match[4]?.split('.').map((part) => {
    if (!/^[0-9]+$/.test(part)) return part
    if (part.length > 1 && part.startsWith('0')) {
      throw new ReleaseError(`numeric prerelease identifier has a leading zero: ${value}`)
    }
    return BigInt(part)
  }) ?? []
  return { core, prerelease }
}

export function compareVersions(left: SemVer, right: SemVer): number {
  for (const index of [0, 1, 2] as const) {
    if (left.core[index] !== right.core[index]) return left.core[index] < right.core[index] ? -1 : 1
  }
  if (!left.prerelease.length || !right.prerelease.length) {
    return Number(!left.prerelease.length) - Number(!right.prerelease.length)
  }
  for (let index = 0; index < Math.min(left.prerelease.length, right.prerelease.length); index++) {
    const first = left.prerelease[index]
    const second = right.prerelease[index]
    if (first === undefined || second === undefined) throw new ReleaseError('incomplete SemVer prerelease')
    if (first === second) continue
    if (typeof first !== typeof second) return typeof first === 'bigint' ? -1 : 1
    return first < second ? -1 : 1
  }
  return Math.sign(left.prerelease.length - right.prerelease.length)
}

export function tagVersion(tag: string): string {
  if (!tag.startsWith('v')) throw new ReleaseError(`release tag must start with v: ${JSON.stringify(tag)}`)
  const value = tag.slice(1)
  versionKey(value)
  return value
}

/** Build metadata is excluded from artifact names so one version names one archive. */
export function withoutBuildMetadata(value: string): string {
  versionKey(value)
  const separator = value.indexOf('+')
  return separator === -1 ? value : value.slice(0, separator)
}
