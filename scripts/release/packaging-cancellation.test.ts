import { expect } from '@std/expect'
import { afterEach, beforeEach, test } from '@std/testing/bdd'
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { buildAll, packageBinary } from './packaging.ts'
import { archiveName, TARGETS } from './targets.ts'
import { executableForTarget } from './testing/executables.ts'

let root: string
let output: string
const version = '1.0.0'
// A non-host target keeps the packaging assertions independent of the local toolchain.
const targetName = process.platform === 'win32' ? 'x86_64-unknown-linux-musl' : 'x86_64-pc-windows-msvc'
const target = TARGETS[targetName]
const archive = archiveName(targetName, version)
const previousArchive = 'previous verified release archive'
const previousChecksums = 'previous complete checksums'

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'tola-package-cancel-test-'))
  output = join(root, 'release')
  await mkdir(output)
  await writeFile(join(output, archive), previousArchive)
  await writeFile(join(output, 'checksums.txt'), previousChecksums)
})
afterEach(async () => {
  await rm(root, { recursive: true, force: true })
})

test('a pre-cancelled build never changes the release set', async () => {
  const controller = new AbortController()
  const reason = new Error('cancelled before build')
  controller.abort(reason)
  await expect(buildAll(version, { root, output, targets: [targetName] }, controller.signal)).rejects.toBe(
    reason,
  )
  expect((await readdir(output)).sort()).toEqual(['checksums.txt', archive].sort())
  expect(await readFile(join(output, archive), 'utf8')).toBe(previousArchive)
  expect(await readFile(join(output, 'checksums.txt'), 'utf8')).toBe(previousChecksums)
})

test('cancellation during source inspection cannot publish or leave staging output', async () => {
  const source = join(root, target.binary)
  await writeFile(source, executableForTarget(target))
  const controller = new AbortController()
  const reason = new Error('cancelled during source inspection')
  const operation = packageBinary(source, output, targetName, version, controller.signal)
  // The initial file inspection yields before output preparation; no timer or filesystem event race is needed.
  queueMicrotask(() => controller.abort(reason))
  await expect(operation).rejects.toBe(reason)
  expect((await readdir(output)).sort()).toEqual(['checksums.txt', archive].sort())
  expect(await readFile(join(output, archive), 'utf8')).toBe(previousArchive)
  expect(await readFile(join(output, 'checksums.txt'), 'utf8')).toBe(previousChecksums)
})
