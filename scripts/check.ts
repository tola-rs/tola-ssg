/**
 * @module
 * Prove every maintenance entry point starts. Each release command must answer `--help` in a fresh
 * process with no permissions granted. This catches import-time work, broken argument wiring, and
 * a package that stopped resolving — failures the unit tests cannot see because they import modules
 * rather than start commands.
 */

import { PACKAGE_ROOT } from './paths.ts'

/** Release commands whose `--help` must answer without a tag, a token, a registry, or a site. */
const RELEASE_ENTRY_POINTS = [
  'release/version.ts',
  'release/check.ts',
  'release/notes.ts',
  'release/package.ts',
  'release/build.ts',
  'release/publish.ts',
]

/** Run one `deno` invocation from this package with inherited stdio; a non-zero exit fails. */
function runDeno(args: readonly string[]): void {
  const completed = new Deno.Command(Deno.execPath(), {
    args: [...args],
    cwd: PACKAGE_ROOT,
    stdin: 'null',
    stdout: 'inherit',
    stderr: 'inherit',
  }).outputSync()
  if (!completed.success) throw new Error(`deno ${args.join(' ')} failed (${completed.code})`)
}

for (const entryPoint of RELEASE_ENTRY_POINTS) {
  // `--no-prompt` turns a permission an entry point only needs at import time into this failure
  // rather than a prompt nobody is watching.
  runDeno(['run', '--no-prompt', entryPoint, '--help'])
}
