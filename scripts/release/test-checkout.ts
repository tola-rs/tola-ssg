import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { run } from './command.ts'

export const WORKSPACE_MANIFEST = `[workspace]
members = [".", "types"]
[workspace.package]
version = "0.8.0" # release version
[workspace.dependencies]
types = { package = "release-types", version = "0.8.0", path = "types", default-features = false }
[package]
name = "release-cli"
version.workspace = true
edition = "2021"
[dependencies]
types.workspace = true
[target.'cfg(unix)'.build-dependencies]
types.workspace = true
`

/**
 * The empty file every checkout uses as its global Git config.
 *
 * Pointing `GIT_CONFIG_GLOBAL` at the platform null device fails on Windows, where Git cannot open
 * `\\.\nul` as a config file (`unable to access '\.\nul': Invalid argument`); an empty regular file
 * isolates the checkout from the developer's global config on every platform.
 */
function emptyGlobalConfigPath(): string {
  const directory = mkdtempSync(join(tmpdir(), 'tola-git-config-'))
  const path = join(directory, 'empty')
  writeFileSync(path, '')
  return path
}

/** The process-wide empty global config; `git()` runs once per test process with stable inputs. */
const GLOBAL_CONFIG = emptyGlobalConfigPath()

export function git(root: string, ...args: string[]): string {
  const result = spawnSync(
    'git',
    [
      '-c',
      'user.name=Release checks',
      '-c',
      'user.email=release@example.invalid',
      '-c',
      'commit.gpgsign=false',
      '-c',
      'core.autocrlf=false',
      ...args,
    ],
    {
      cwd: root,
      encoding: 'utf8',
      env: {
        ...process.env,
        GIT_CONFIG_GLOBAL: GLOBAL_CONFIG,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_COUNT: '0',
        GIT_TERMINAL_PROMPT: '0',
      },
    },
  )
  if (result.error || result.status !== 0) {
    throw new Error(`checkout git ${args.join(' ')} failed: ${result.error?.message ?? result.stderr}`)
  }
  return result.stdout.trim()
}

export async function withGitCheckout<T>(operation: (root: string) => T | Promise<T>): Promise<T> {
  const root = mkdtempSync(join(tmpdir(), 'tola-release-test-'))
  try {
    git(root, 'init', '-q')
    git(root, 'config', 'core.autocrlf', 'false')
    git(root, 'config', 'core.hooksPath', join(root, '.git', 'disabled-hooks'))
    writeFileSync(join(root, 'Cargo.toml'), WORKSPACE_MANIFEST)
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'checkout')
    return await operation(root)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

export function withWorkspace<T>(operation: (root: string) => T | Promise<T>): Promise<T> {
  return withGitCheckout(async (root) => {
    mkdirSync(join(root, 'src'))
    mkdirSync(join(root, 'types', 'src'), { recursive: true })
    writeFileSync(join(root, 'src', 'main.rs'), 'fn main() {}\n')
    writeFileSync(
      join(root, 'types', 'Cargo.toml'),
      '[package]\nname = "release-types"\nversion.workspace = true\nedition = "2021"\n',
    )
    writeFileSync(join(root, 'types', 'src', 'lib.rs'), '')
    run(root, ['cargo', 'metadata', '--offline', '--format-version', '1'])
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'workspace')
    return await operation(root)
  })
}

/** Write the two files a version update rewrites, with the bytes it reads and the bytes it writes. */
export function versionFiles(root: string): {
  originals: Map<string, Buffer>
  replacements: Map<string, Buffer>
} {
  const originals = new Map([
    ['Cargo.toml', Buffer.from('original manifest')],
    ['Cargo.lock', Buffer.from('original lock')],
  ])
  const replacements = new Map([
    ['Cargo.toml', Buffer.from('new manifest')],
    ['Cargo.lock', Buffer.from('new lock')],
  ])
  for (const [path, content] of originals) writeFileSync(join(root, path), content)
  return { originals, replacements }
}
