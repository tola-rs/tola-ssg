import { withoutBuildMetadata } from './semver.ts'

/** One published release artifact. The name is the Rust target triple Cargo builds. */
export interface Target {
  readonly system: 'linux' | 'darwin' | 'windows'
  readonly machine: 'x86_64' | 'aarch64'
  readonly binary: 'tola' | 'tola.exe'
}

export const TARGETS = {
  'x86_64-unknown-linux-musl': {
    system: 'linux',
    machine: 'x86_64',
    binary: 'tola',
  },
  'aarch64-unknown-linux-musl': {
    system: 'linux',
    machine: 'aarch64',
    binary: 'tola',
  },
  'aarch64-apple-darwin': {
    system: 'darwin',
    machine: 'aarch64',
    binary: 'tola',
  },
  'x86_64-pc-windows-msvc': {
    system: 'windows',
    machine: 'x86_64',
    binary: 'tola.exe',
  },
} as const satisfies Record<string, Target>

export type TargetName = keyof typeof TARGETS
export const TARGET_NAMES: readonly TargetName[] = Object.keys(TARGETS) as TargetName[]

export function targetName(value: string): TargetName {
  if (!Object.hasOwn(TARGETS, value)) {
    throw new Error(`unknown target: ${value} (choose from ${TARGET_NAMES.join(', ')})`)
  }
  return value as TargetName
}

/** The versioned, conventionally named archive a target publishes. */
export function archiveName(name: TargetName, version: string): string {
  const extension = TARGETS[name].system === 'windows' ? 'zip' : 'tar.gz'
  return `tola-${withoutBuildMetadata(version)}-${name}.${extension}`
}

/** The one release target this host builds natively; every other target belongs to CI. */
export function hostTargetName(): TargetName {
  const system = process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
  const machine = process.arch === 'arm64' ? 'aarch64' : process.arch === 'x64' ? 'x86_64' : undefined
  const match = TARGET_NAMES.find(
    (name) => TARGETS[name].system === system && TARGETS[name].machine === machine,
  )
  if (match === undefined) {
    throw new Error(
      `no release target is built on this host (${process.platform}/${process.arch}); ` +
        `supported hosts are ${TARGET_NAMES.join(', ')}`,
    )
  }
  return match
}
