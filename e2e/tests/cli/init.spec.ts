import { expect } from '@playwright/test'
import { mkdir, readdir, readFile, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { COMMAND_TIMEOUT_MS, commandRunner, expectExited, test } from '../../support/process.ts'

const INITIALIZE = `def main [title: string = 'A new site', --invalid, --cancel] {
  if $cancel { exit 130 }
  let alias = $env.TOLA_MODULE_NAME
  let enabled = if $invalid { 'wrong type' } else { true }
  {site: {title: $title},
   modules: {($alias): {path: $env.TOLA_MODULE_ROOT, options: {enabled: $enabled}}},
   theme: {module: $alias}}
  | to toml | save tola.toml
  mkdir content
  '= Welcome' | save content/index.typ
}
`

async function writeModule(root: string): Promise<void> {
  await mkdir(join(root, 'tools'), { recursive: true })
  await writeFile(
    join(root, 'tola-module.toml'),
    `schema-version = 1
name = "starter"
version = "1.0.0"
[site]
entry = "site.typ"
[options]
schema = "options.typ"
export = "schema"
[init]
argv = ["{tola}", "nu", "tools/init.nu"]
`,
  )
  await writeFile(join(root, 'site.typ'), '#document("index.html", format: "html")[Hello]\n')
  await writeFile(
    join(root, 'options.typ'),
    '#import "@tola/schema:0.0.0" as s\n#let schema = s.schema((enabled: s.optional(bool, default: true),))\n',
  )
  await writeFile(join(root, 'tools/init.nu'), INITIALIZE)
}

test('initialized site publishes sources without host packages', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const site = join(root, 'new site')
  expectExited(await run(['init', site, '--pure'], root))
  expect((await readdir(site)).sort()).toEqual(['.gitignore', '.ignore', 'content', 'site.typ', 'tola.toml'])
  await writeFile(join(site, 'content/index.typ'), '= Welcome\n')
  expectExited(await run(['build', '--pure'], site, COMMAND_TIMEOUT_MS.build))
  expect(await readFile(join(site, 'public/index.html'), 'utf8')).toContain('Welcome')
  expect(await readFile(join(site, 'public/404.html'), 'utf8')).toContain('Page not found')
})

test('forced init preserves existing files', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeFile(join(root, 'notes.txt'), 'Keep my notes')
  expectExited(await run(['init'], root), 1)
  expectExited(await run(['init', '--force'], root))
  const configuration = await readFile(join(root, 'tola.toml'), 'utf8')
  expectExited(await run(['init', '--force'], root), 1)
  expect(await readFile(join(root, 'tola.toml'), 'utf8')).toBe(configuration)
  expect(await readFile(join(root, 'notes.txt'), 'utf8')).toBe('Keep my notes')
})

test('init dry-run leaves the target absent', async ({ binary, directory: root }) => {
  expectExited(await commandRunner(binary)(['init', 'preview', '--dry-run'], root))
  expect(await readdir(root)).toEqual([])
})

test('module init resolves paths from the final site', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const module = join(root, 'source module')
  await writeModule(module)
  const script = INITIALIZE.replace('$env.TOLA_MODULE_ROOT, options:', "'../source module', options:")
  await writeFile(join(module, 'tools/init.nu'), script)
  expectExited(
    await run(['init', 'author site', '--from', module, '--alias', 'forest', '--', 'Author title'], root),
  )
  const site = join(root, 'author site')
  expect(await readFile(join(site, 'tola.toml'), 'utf8')).toContain('Author title')
  const configuration = await run(['inspect', 'config'], site)
  expectExited(configuration)
  const settings = JSON.parse(configuration.stdout)
  expect(settings.theme).toBe('forest')
  expect(await realpath(settings.modules.forest.root)).toBe(await realpath(module))
  expectExited(await run(['build', '--offline'], site, COMMAND_TIMEOUT_MS.build))
  expect(await readFile(join(site, 'public/index.html'), 'utf8')).toContain('Hello')
  expect(await readdir(site)).not.toContain('tools')
})

for (const [option, code] of [['--invalid', 1], ['--cancel', 130]] as const) {
  test('module init ' + option + ' leaves the target absent', async ({ binary, directory: root }) => {
    await writeModule(join(root, 'module'))
    const result = await commandRunner(binary)(['init', 'site', '--from', 'module', '--', option], root)
    expectExited(result, code)
    expect(await readdir(root)).toEqual(['module'])
  })
}

test('module dry-run leaves the target absent', async ({ binary, directory: root }) => {
  await writeModule(join(root, 'module'))
  expectExited(await commandRunner(binary)(['init', 'site', '--from', 'module', '--dry-run'], root))
  expect(await readdir(root)).toEqual(['module'])
})

test('pure init uses the final site boundary', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeModule(join(root, 'external'))
  await writeFile(
    join(root, 'external/tools/init.nu'),
    "'ran' | save ($env.TOLA_MODULE_ROOT | path join invoked)\n" + INITIALIZE,
  )
  expectExited(await run(['init', 'site', '--from', 'external', '--pure'], root), 1)
  await expect(readFile(join(root, 'external/invoked'))).rejects.toMatchObject({ code: 'ENOENT' })
  await expect(readdir(join(root, 'site'))).rejects.toMatchObject({ code: 'ENOENT' })
  await writeModule(join(root, 'existing/theme'))
  expectExited(await run(['init', 'existing', '--force', '--from', 'existing/theme', '--pure'], root))
  expectExited(await run(['build', '--pure'], join(root, 'existing'), COMMAND_TIMEOUT_MS.build))
})

test('configured module alias initializes a new site', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeModule(join(root, 'module'))
  await writeFile(join(root, 'tola.toml'), "[modules.forest]\npath = 'module'\n")
  expectExited(await run(['forest', 'init', 'site', '--', 'Through alias'], root))
  const configuration = await readFile(join(root, 'site/tola.toml'), 'utf8')
  expect(configuration).toContain('Through alias')
  expectExited(await run(['forest', 'init', 'site', '--force'], root), 1)
  expect(await readFile(join(root, 'site/tola.toml'), 'utf8')).toBe(configuration)
})

test('initializer schema errors retain the module source', async ({ binary, directory: root }) => {
  await writeModule(join(root, 'module'))
  await writeFile(
    join(root, 'module/options.typ'),
    '#import "@tola/schema:0.0.0" as s\n#let schema = s.schema((enabled: s.refine(bool, value => panic("initializer schema callback"), message: "invalid"),))\n',
  )
  const result = await commandRunner(binary)(['init', 'site', '--from', 'module'], root)
  expectExited(result, 1)
  expect(result.stderr).toContain('initializer schema callback')
  expect(result.stderr).toContain('options.typ')
  expect(await readdir(root)).toEqual(['module'])
})

test('initializer must produce a configuration', async ({ binary, directory: root }) => {
  await writeModule(join(root, 'module'))
  await writeFile(join(root, 'module/tools/init.nu'), 'def main [] {}\n')
  expectExited(await commandRunner(binary)(['init', 'site', '--from', 'module'], root), 1)
  expect(await readdir(root)).toEqual(['module'])
})

test('generated modules resolve before initialization commits', async ({ binary, directory: root }) => {
  await writeModule(join(root, 'module'))
  const script = INITIALIZE.replace(
    'options: {enabled: $enabled}}}',
    "options: {enabled: $enabled}}, generated: {path: 'generated'}}",
  )
  await writeFile(
    join(root, 'module/tools/init.nu'),
    "mkdir generated\n'schema-version = 1' | save generated/tola-module.toml\n" + script,
  )
  const run = commandRunner(binary)
  expectExited(await run(['init', 'site', '--from', 'module'], root))
  expectExited(await run(['generated', '--help'], join(root, 'site')))
})
