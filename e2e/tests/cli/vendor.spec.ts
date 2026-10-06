import { expect } from '@playwright/test'
import { createHash } from 'node:crypto'
import { lstat, mkdir, readdir, readFile, rename, rm, symlink, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { startGate } from '../../support/gate.ts'
import { hookToml } from '../../support/hooks.ts'
import {
  COMMAND_TIMEOUT_MS,
  type CommandRunner,
  commandRunner,
  expectExited,
  startCommand,
  test,
} from '../../support/process.ts'
import { writeMinimalSite } from '../../support/site.ts'

async function writeHostPackage(root: string, name: string, version: string, body: string) {
  const location = join(root, 'host-packages', 'preview', name, version)
  await mkdir(location, { recursive: true })
  await writeFile(
    join(location, 'typst.toml'),
    `[package]\nname = "${name}"\nversion = "${version}"\nentrypoint = "lib.typ"\n`,
  )
  await writeFile(join(location, 'lib.typ'), body)
  return join(root, 'host-packages')
}

/** Every entry under `root` by relative path, and each file's raw bytes, in that order. */
async function treeContents(root: string): Promise<{ entries: string[]; files: Buffer[] }> {
  const entries: string[] = []
  const files: Buffer[] = []
  const visit = async (directory: string, prefix: string) => {
    const children = await readdir(directory, { withFileTypes: true })
    for (const child of children.sort((left, right) => left.name.localeCompare(right.name))) {
      const path = join(directory, child.name)
      const relative = join(prefix, child.name)
      if (child.isDirectory()) {
        entries.push(`${relative}/`)
        await visit(path, relative)
      } else {
        entries.push(relative)
        files.push(await readFile(path))
      }
    }
  }
  await visit(root, '')
  return { entries, files }
}

async function writeThemeSite(root: string) {
  await writeMinimalSite(root, {
    program: '#import "@preview/theme:1.0.0": accent\n#document("index.html")[#accent]\n',
  })
  await writeFile(join(root, 'tola.toml'), '[vendor]\npath = "vendor"\n')
  return writeHostPackage(root, 'theme', '1.0.0', '#let accent = "vendored"\n')
}

/** Writes the theme site and vendors `host` into it, returning the host package root. */
async function vendorThemeSite(root: string, run: CommandRunner): Promise<string> {
  const host = await writeThemeSite(root)
  expectExited(await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build))
  return host
}

/** A pure build of the site publishes a page containing `text`. */
async function expectPureBuildPublishes(
  root: string,
  run: CommandRunner,
  text: string,
): Promise<void> {
  expectExited(await run(['build', '--pure'], root, COMMAND_TIMEOUT_MS.build))
  expect(await readFile(join(root, 'public/index.html'), 'utf8')).toContain(text)
}

test('vendor freezes transitive package resources', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root, {
    program: '#import "@local/bridge:1.0.0": accent\n#document("index.html")[#accent]\n',
  })
  await writeFile(join(root, 'tola.toml'), '[vendor]\npath = "vendor"\n')
  const host = await writeHostPackage(root, 'theme', '1.0.0', '#let accent = json("colors.json").accent\n')
  await writeFile(join(host, 'preview/theme/1.0.0/colors.json'), '{"accent":"vendored resource"}')
  const bridge = join(host, 'local/bridge/1.0.0')
  await mkdir(bridge, { recursive: true })
  await writeFile(
    join(bridge, 'typst.toml'),
    '[package]\nname = "bridge"\nversion = "1.0.0"\nentrypoint = "lib.typ"\n',
  )
  await writeFile(join(bridge, 'lib.typ'), '#import "@preview/theme:1.0.0": accent\n')

  const vendoring = await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build)
  expectExited(vendoring)

  expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/colors.json'), 'utf8'))
    .toBe('{"accent":"vendored resource"}')
  await expect(lstat(join(root, 'public'))).rejects.toMatchObject({ code: 'ENOENT' })
  await rm(host, { recursive: true })

  await expectPureBuildPublishes(root, run, 'vendored resource')
})

test('normal vendoring retains selected bytes', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const host = await vendorThemeSite(root, run)
  await writeFile(join(host, 'preview/theme/1.0.0/lib.typ'), '#let accent = "changed host"\n')
  await writeFile(join(root, 'vendor/notes.txt'), 'author-owned')

  const again = await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build)
  expectExited(again)
  await expect(
    readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'),
  ).resolves.toContain('vendored')
  expect(await readFile(join(root, 'vendor/notes.txt'), 'utf8')).toBe('author-owned')
})

test('vendored icons serve a pure build', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const collection = JSON.stringify({
    prefix: 'ui',
    icons: { mark: { body: '<path fill="currentColor" d="M0 0h16v16H0z"/>' } },
  })
  const sha256 = createHash('sha256').update(collection).digest('hex')
  const gate = await startGate((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(collection)
  })
  try {
    await writeMinimalSite(root, {
      program:
        '#import "@tola/icon:0.0.0": icon\n#document("index.html", format: "html")[#icon("ui:mark")]\n',
    })
    await writeFile(
      join(root, 'tola.toml'),
      `[vendor]\npath = "vendor"\n\n[icons.collections.ui]\nsource-type = "remote-json"\nurl = "${gate.url}/ui.json"\nsha256 = "${sha256}"\n`,
    )

    const vendoring = await run(['vendor'], root, COMMAND_TIMEOUT_MS.build)
    expectExited(vendoring)
    expect(await readFile(join(root, 'vendor/icons/ui.json'), 'utf8')).toBe(collection)

    await rm(join(root, '.tola'), { recursive: true, force: true })
    await expectPureBuildPublishes(root, run, '<svg')
  } finally {
    await gate.close()
  }
})

test('refresh installs replacement bytes', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const host = await vendorThemeSite(root, run)
  await writeFile(join(host, 'preview/theme/1.0.0/lib.typ'), '#let accent = "replacement"\n')

  expectExited(await run(['vendor', '--refresh', '--package-path', host], root, COMMAND_TIMEOUT_MS.build))

  expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
    .toBe('#let accent = "replacement"\n')
  await expectPureBuildPublishes(root, run, 'replacement')
})

test('dry run preserves current dependencies', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const host = await vendorThemeSite(root, run)
  await writeFile(join(host, 'preview/theme/1.0.0/lib.typ'), '#let accent = "replacement"\n')

  expectExited(
    await run(['vendor', '--refresh', '--dry-run', '--package-path', host], root, COMMAND_TIMEOUT_MS.build),
  )

  expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
    .toBe('#let accent = "vendored"\n')
  await expect(lstat(join(root, 'public'))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('dry run refuses an interrupted installation', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const host = await vendorThemeSite(root, run)

  // What an interrupted installation leaves: the prepared tree already replaced the vendor tree,
  // and the previous tree stays in the workspace with its journal until `committed` is written.
  const workspace = join(root, '.vendor-vendor')
  await mkdir(join(workspace, 'candidate/fonts'), { recursive: true })
  await mkdir(join(workspace, 'candidate/icons'), { recursive: true })
  await mkdir(join(workspace, 'previous'), { recursive: true })
  await writeFile(join(workspace, 'owner'), 'tola-vendor-root\nvendor=vendor/\n')
  await writeFile(join(workspace, 'replacing'), '111')
  await rename(join(root, 'vendor/typst-packages'), join(workspace, 'previous/typst-packages'))
  await mkdir(join(root, 'vendor/typst-packages/preview/theme/1.0.0'), { recursive: true })
  await writeFile(
    join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'),
    '#let accent = "uncommitted"\n',
  )

  const vendorContents = await treeContents(join(root, 'vendor'))
  const recoveryContents = await treeContents(workspace)
  expectExited(await run(['vendor', '--dry-run', '--package-path', host], root, COMMAND_TIMEOUT_MS.build), 1)
  expect(await treeContents(join(root, 'vendor'))).toEqual(vendorContents)
  expect(await treeContents(workspace)).toEqual(recoveryContents)

  expectExited(await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build))
  await expect(lstat(workspace)).rejects.toMatchObject({ code: 'ENOENT' })
  expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
    .toBe('#let accent = "vendored"\n')
  await expectPureBuildPublishes(root, run, 'vendored')
})

test('failed refresh preserves usable inputs', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Creating source links requires Unix symlink permissions.')
  const run = commandRunner(binary)
  const host = await vendorThemeSite(root, run)
  await writeFile(join(host, 'preview/theme/1.0.0/lib.typ'), '#let accent = "replacement"\n')
  await symlink('missing-resource', join(host, 'preview/theme/1.0.0/unused.txt'))

  for (const flags of [[], ['--dry-run']]) {
    const refresh = await run(
      ['vendor', '--refresh', ...flags, '--package-path', host],
      root,
      COMMAND_TIMEOUT_MS.build,
    )
    expectExited(refresh, 1)
    expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
      .toBe('#let accent = "vendored"\n')
  }
  await expectPureBuildPublishes(root, run, 'vendored')
})

test('pure verification preserves previous inputs', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Creating source links requires Unix symlink permissions.')
  const run = commandRunner(binary)
  const site = join(root, 'site')
  const host = await vendorThemeSite(site, run)
  await writeFile(join(host, 'preview/theme/1.0.0/lib.typ'), '#let accent = "replacement"\n')
  await writeFile(join(root, 'external.txt'), 'host-only bytes')
  await symlink(join(root, 'external.txt'), join(site, 'external.txt'))
  await writeFile(
    join(site, 'site.typ'),
    '#import "@preview/theme:1.0.0": accent\n#document("index.html")[#accent #read("external.txt")]\n',
  )

  expectExited(await run(['vendor', '--refresh', '--package-path', host], site, COMMAND_TIMEOUT_MS.build), 1)

  expect(await readFile(join(site, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
    .toBe('#let accent = "vendored"\n')
})

test('failed first copy never shadows host', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Creating source links requires Unix symlink permissions.')
  const run = commandRunner(binary)
  const host = await writeThemeSite(root)
  const broken = join(host, 'preview/theme/1.0.0/unused.txt')
  await symlink('missing-resource', broken)

  expectExited(await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build), 1)
  await expect(lstat(join(root, 'vendor/typst-packages'))).rejects.toMatchObject({ code: 'ENOENT' })
  await rm(broken)

  expectExited(await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build))
  await expectPureBuildPublishes(root, run, 'vendored')
})

test('symlinked destinations remain untouched', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Creating destination links requires Unix symlink permissions.')
  const run = commandRunner(binary)
  for (const target of ['root', 'packages']) {
    const site = join(root, target)
    let host = await writeThemeSite(site)
    if (target === 'root') {
      const frozen = join(site, 'frozen')
      await mkdir(frozen)
      await rename(host, join(frozen, 'typst-packages'))
      host = join(frozen, 'typst-packages')
      await symlink(frozen, join(site, 'vendor'))
    } else {
      await mkdir(join(site, 'vendor'))
      await symlink(host, join(site, 'vendor/typst-packages'))
    }

    expectExited(
      await run(['vendor', '--refresh', '--package-path', host], site, COMMAND_TIMEOUT_MS.build),
      1,
    )
    expect(await readFile(join(host, 'preview/theme/1.0.0/lib.typ'), 'utf8'))
      .toBe('#let accent = "vendored"\n')
    expect((await lstat(join(site, target === 'root' ? 'vendor' : 'vendor/typst-packages'))).isSymbolicLink())
      .toBe(true)
  }
})

test('pure refresh refuses host replacement', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const host = await writeThemeSite(root)
  expectExited(await run(['vendor', '--offline', '--package-path', host], root, COMMAND_TIMEOUT_MS.build))
  await writeFile(join(host, 'preview/theme/1.0.0/lib.typ'), '#let accent = "replacement"\n')

  expectExited(
    await run(['vendor', '--pure', '--refresh', '--package-path', host], root, COMMAND_TIMEOUT_MS.build),
    1,
  )

  expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
    .toBe('#let accent = "vendored"\n')
})

test('restricted vendor never fetches icons', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  let requests = 0
  const gate = await startGate((_request, response) => {
    requests += 1
    response.writeHead(500)
    response.end()
  })
  try {
    for (const scope of ['--offline', '--pure']) {
      const site = join(root, scope.slice(2))
      await writeMinimalSite(site, {
        program:
          '#import "@tola/icon:0.0.0": icon\n#document("index.html", format: "html")[#icon("ui:mark")]\n',
      })
      await writeFile(
        join(site, 'tola.toml'),
        `[vendor]\npath = "vendor"\n\n[icons.collections.ui]\nsource-type = "remote-json"\nurl = "${gate.url}/ui.json"\nsha256 = "${
          '0'.repeat(64)
        }"\n`,
      )
      expectExited(await run(['vendor', scope], site, COMMAND_TIMEOUT_MS.build), 1)
    }
    expect(requests).toBe(0)
  } finally {
    await gate.close()
  }
})

test('vendor skips hooks', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const host = await writeThemeSite(root)
  const hook = join(root, 'refuse-vendoring.mjs')
  await writeFile(
    hook,
    `
    import { writeFileSync } from 'node:fs';
    writeFileSync('hook-ran', 'unexpected');
    process.exit(9);
  `,
  )
  const hooks = (['before-build', 'generate-outputs', 'after-publish'] as const)
    .map((phase) =>
      hookToml(phase, hook, {
        name: phase,
        ...(phase === 'before-build' ? { outputs: ['hook-ran'] } : {}),
        ...(phase === 'generate-outputs' ? { outputs: [{ file: 'hook-ran' }] } : {}),
      })
    ).join('\n')
  await writeFile(join(root, 'tola.toml'), `[vendor]\npath = "vendor"\n\n${hooks}`)

  expectExited(await run(['vendor', '--package-path', host], root, COMMAND_TIMEOUT_MS.build))

  await expect(lstat(join(root, 'hook-ran'))).rejects.toMatchObject({ code: 'ENOENT' })
  await expect(lstat(join(root, 'public'))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('cancelled refresh preserves usable inputs', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Node pipes cannot send console interrupts to Windows children.')
  const run = commandRunner(binary)
  const host = await vendorThemeSite(root, run)
  let requested = false
  const gate = await startGate((_request, response) => {
    requested = true
    response.writeHead(200, { 'content-type': 'application/json' })
    response.write('{"prefix":"ui","icons":{')
  })
  const config = '[vendor]\npath = "vendor"\n'
  await writeFile(
    join(root, 'tola.toml'),
    `${config}\n[icons.collections.ui]\nsource-type = "remote-json"\nurl = "${gate.url}/ui.json"\nsha256 = "${
      '0'.repeat(64)
    }"\n`,
  )
  const command = startCommand(binary, ['vendor', '--refresh', '--package-path', host], root)
  try {
    await expect.poll(() => requested).toBe(true)
    command.command.interrupt()
    expect((await command.command.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(130)
    expect(await readFile(join(root, 'vendor/typst-packages/preview/theme/1.0.0/lib.typ'), 'utf8'))
      .toBe('#let accent = "vendored"\n')
  } finally {
    await gate.close()
    await command.command.terminate()
  }
  await writeFile(join(root, 'tola.toml'), config)
  await expectPureBuildPublishes(root, run, 'vendored')
})
