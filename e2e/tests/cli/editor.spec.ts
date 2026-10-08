import { expect } from '@playwright/test'
import { mkdir, readdir, readFile, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { COMMAND_TIMEOUT_MS, commandRunner, expectExited, test } from '../../support/process.ts'
import { writeMinimalSite } from '../../support/site.ts'

/** The author's own JSONC: the merge must keep both the comment and the unrelated setting. */
const SETTINGS = `{
  // Kept by the merge.
  "editor.formatOnSave": true
}
`

const MIRRORED_VERSION = '.tola/builtin-packages/tola/source/0.0.0'
const OBSOLETE_VERSION = '.tola/builtin-packages/tola/source/9.9.9'
const PACKAGE_LINK = '.tola/editor-packages/tola/source/0.0.0'
const PACKAGE_VIEW = '.tola/editor-packages'
const THEME_DIRECTORY = '.tola/builtin-packages/tola/code/0.0.0/code-themes'

async function writeEditorSite(root: string): Promise<void> {
  await writeMinimalSite(root)
  await mkdir(join(root, '.vscode'), { recursive: true })
  await writeFile(join(root, '.vscode/settings.json'), SETTINGS)
  await mkdir(join(root, OBSOLETE_VERSION), { recursive: true })
  await writeFile(join(root, OBSOLETE_VERSION, 'lib.typ'), 'Superseded package version.\n')
}

test('editor setup dry-run writes nothing', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeEditorSite(root)

  const preview = await run(
    ['editor', 'setup', '--dry-run', 'vscode', 'neovim'],
    root,
    COMMAND_TIMEOUT_MS.standard,
  )
  expectExited(preview)
  const report = preview.stdout + preview.stderr
  expect(report).toContain('// Kept by the merge.')
  expect(report).toContain('"editor.formatOnSave": true')
  expect(report).toContain('tola-lsp')
  expect(report).toContain(OBSOLETE_VERSION)
  expect(report).toContain(PACKAGE_VIEW)

  expect(await readFile(join(root, '.vscode/settings.json'), 'utf8')).toBe(SETTINGS)
  await expect(readdir(join(root, PACKAGE_VIEW))).rejects.toMatchObject({ code: 'ENOENT' })
  await expect(readdir(join(root, MIRRORED_VERSION))).rejects.toMatchObject({ code: 'ENOENT' })
  expect(await readFile(join(root, OBSOLETE_VERSION, 'lib.typ'), 'utf8'))
    .toBe('Superseded package version.\n')
})

test('editor setup applies its previewed changes', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeEditorSite(root)

  const preview = await run(
    ['editor', 'setup', '--dry-run', 'vscode', 'neovim'],
    root,
    COMMAND_TIMEOUT_MS.standard,
  )
  expectExited(preview)
  const applied = await run(['editor', 'setup', 'vscode', 'neovim'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(applied)

  const settings = await readFile(join(root, '.vscode/settings.json'), 'utf8')
  expect(settings).toContain('// Kept by the merge.')
  expect(settings).toContain('"editor.formatOnSave": true')
  expect(settings).toContain('"tola.configPath"')
  const report = preview.stdout + preview.stderr
  expect(report).toContain(settings)
  const appliedReport = applied.stdout + applied.stderr
  expect(appliedReport).toContain('tola-lsp')
  expect(appliedReport).toContain('`.vscode/settings.json` updated')

  const repeated = await run(['editor', 'setup', 'vscode'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(repeated)
  expect(repeated.stdout + repeated.stderr).toContain('`.vscode/settings.json` already current')
  expect(await readdir(join(root, MIRRORED_VERSION))).toContain('typst.toml')
  expect(await realpath(join(root, PACKAGE_LINK))).toBe(await realpath(join(root, MIRRORED_VERSION)))
  await expect(readdir(join(root, OBSOLETE_VERSION))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('editor packages refreshes packages without merging settings', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeEditorSite(root)

  const refreshed = await run(['editor', 'packages'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(refreshed)

  expect(await readdir(join(root, MIRRORED_VERSION))).toContain('typst.toml')
  expect(await readdir(join(root, THEME_DIRECTORY))).toContain('zenburn.tmTheme')
  expect(await realpath(join(root, PACKAGE_LINK))).toBe(await realpath(join(root, MIRRORED_VERSION)))
  await expect(readdir(join(root, OBSOLETE_VERSION))).rejects.toMatchObject({ code: 'ENOENT' })
  expect(await readFile(join(root, '.vscode/settings.json'), 'utf8')).toBe(SETTINGS)
})

test('editor template prints where its settings go', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const template = await run(['editor', 'template', 'helix'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(template)
  expect(template.stderr).toContain('.helix/languages.toml')
  expect(template.stdout).toContain('tola-lsp')
  expect(template.stdout).not.toContain('.helix/languages.toml')
})

test('editor setup omits configuration and discovered packages', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const report = await run(
    ['editor', 'setup', '--dry-run', 'vscode'],
    root,
    COMMAND_TIMEOUT_MS.standard,
  )
  expectExited(report)
  expect(report.stdout + report.stderr).not.toContain('tola.packagePath')
  expect(report.stdout + report.stderr).not.toContain('tola.configPath')

  const listed = await run(['editor', 'setup', '--list'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(listed)
  // A bare `tola lsp` discovers the host's package roots itself, so the entry names none of them.
  expect(listed.stderr).not.toContain('--package-path')
  expect(listed.stderr).not.toContain('--config')

  const applied = await run(['editor', 'setup', 'vscode'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(applied)
  const settings = await readFile(join(root, '.vscode/settings.json'), 'utf8')
  expect(settings).not.toContain('tola.packagePath')
  expect(settings).not.toContain('tola.packageCachePath')
  expect(settings).not.toContain('tola.configPath')
})

test('editor listing skips package preparation', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  await writeFile(join(root, '.tola'), 'This is not an editor package directory.\n')

  const listed = await run(['editor', 'setup', '--list'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(listed)
  expect(listed.stdout + listed.stderr).toContain('copy settings manually')
  expect(await readFile(join(root, '.tola'), 'utf8'))
    .toBe('This is not an editor package directory.\n')
})

test('editor setup requires selection without terminal', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  await writeFile(join(root, '.tola'), 'This is not an editor package directory.\n')

  const unselected = await run(['editor', 'setup'], root, COMMAND_TIMEOUT_MS.standard)
  expect(unselected.code).not.toBe(0)
  expect(unselected.stdout + unselected.stderr).toContain('no editor specified')
  expect(await readFile(join(root, '.tola'), 'utf8'))
    .toBe('This is not an editor package directory.\n')
})

test('editor setup preserves equivalent configuration paths', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  await mkdir(join(root, '.vscode'))
  const settings = join(root, '.vscode/settings.json')
  const source = '{"tola.configPath": "./tola.toml", "editor.fontSize": 14}\n'
  await writeFile(settings, source)
  expectExited(await run(['editor', 'setup', 'vscode'], root))
  expect(await readFile(settings, 'utf8')).toBe(source)
  expect(await readdir(join(root, MIRRORED_VERSION))).toContain('typst.toml')
})

test('editor setup refuses malformed settings before writing', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  await mkdir(join(root, '.vscode'))
  const settings = join(root, '.vscode/settings.json')
  const source = '{"editor.fontSize": banana}\n'
  await writeFile(settings, source)
  const refused = await run(['editor', 'setup', 'vscode'], root)
  expect(refused.code).not.toBe(0)
  expect(refused.stderr).toContain('editor.configuration')
  expect(await readFile(settings, 'utf8')).toBe(source)
  await expect(readdir(join(root, MIRRORED_VERSION))).rejects.toMatchObject({ code: 'ENOENT' })
})
