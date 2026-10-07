import { expect } from '@std/expect'
import { afterEach, beforeEach, test } from '@std/testing/bdd'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import {
  LICENSE_FILES,
  licenseArchiveName,
  packageBinary,
  verifyDirectory,
  writeLicenseFiles,
} from './packaging.ts'
import { archiveName, TARGETS } from './targets.ts'
import { executableForTarget } from './testing/executables.ts'

let root: string
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'tola-release-licenses-'))
})
afterEach(async () => {
  await rm(root, { recursive: true, force: true })
})

test('staged licences complete the release directory', async () => {
  // Inspect a foreign executable without running it on this host.
  const target = process.platform === 'win32' ? 'x86_64-unknown-linux-musl' : 'x86_64-pc-windows-msvc'
  const source = join(root, 'binary')
  const output = join(root, 'release')
  await writeFile(source, executableForTarget(TARGETS[target]))
  await packageBinary(source, output, target, '1.0.0')
  await expect(verifyDirectory(output, '1.0.0', { target })).rejects.toThrow('missing license files')

  for (const relative of Object.values(LICENSE_FILES)) {
    const path = join(root, relative)
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, `license from ${relative}\n`)
  }
  const licenses = join(root, 'licenses')
  await writeLicenseFiles({ root, output: licenses }, '1.0.0')
  const bundled = licenseArchiveName('1.0.0')
  await writeFile(join(output, bundled), await readFile(join(licenses, bundled)))
  expect([...(await verifyDirectory(output, '1.0.0', { target })).keys()]).toEqual([
    archiveName(target, '1.0.0'),
  ])
})

test('missing license sources stop staging', async () => {
  await expect(writeLicenseFiles({ root, output: join(root, 'licenses') }, '1.0.0')).rejects.toThrow(
    'missing LICENSE',
  )
})
