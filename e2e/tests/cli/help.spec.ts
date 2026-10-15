import { expect } from '@playwright/test'
import { chmod, mkdir, readdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { stripVTControlCharacters } from 'node:util'
import {
  COMMAND_TIMEOUT_MS,
  commandRunner,
  expectExited,
  runCommand,
  startCommand,
  test,
} from '../../support/process.ts'
import { runOnTerminal } from '../../support/terminal.ts'

test('documentation help needs no usable site', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeFile(join(root, 'tola.toml'), 'This is not TOML')
  await writeFile(join(root, 'site.typ'), '#panic("Help must not compile this site")')

  const hooks = await run(['help', '[build.hooks]'], root)
  expectExited(hooks)
  expect(hooks.stdout).toContain('build.hooks')
  expect(hooks.stderr).toBe('')

  const reference = await run(['help', '@tola/document'], root)
  expectExited(reference)
  expect(reference.stdout).toContain('@tola/document:0.0.0')
  expect(reference.stderr).toBe('')

  const selected = await run(['help', '@tola/document', 'headings', 'references'], root)
  expectExited(selected)
  expect(selected.stdout).toContain('let headings(')
  expect(selected.stdout).toContain('references(')

  const rejected = await run(['help', '@tola/document', 'headings', 'unknown-export'], root)
  expectExited(rejected, 1)
  expect(rejected.stdout).toBe('')
  expect(rejected.stderr).toContain('unknown-export')
  expect(rejected.stderr).toContain('headings')
  expect((await readdir(root)).sort()).toEqual(['site.typ', 'tola.toml'])
})

test('documentation color follows stdout choice', async ({ binary, directory: root }) => {
  const run = (color: string) =>
    runCommand(binary, ['--color', color, 'help', '[typst.fonts]'], root, COMMAND_TIMEOUT_MS.standard)
  const plain = await run('never')
  const automatic = await run('auto')
  const colored = await run('always')
  for (const command of [plain, automatic, colored]) expectExited(command)
  expect(plain.stdout).not.toContain('\u001b')
  expect(automatic.stdout).toBe(plain.stdout)
  expect(colored.stdout).toContain('\u001b[')
  expect(stripVTControlCharacters(colored.stdout)).toBe(plain.stdout)
})

test('package documentation follows the language', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)

  const english = startCommand(binary, ['help', '@tola/address', '--color', 'never'], root, {
    LANGUAGE: 'zh_CN.UTF-8',
    LC_ALL: 'zh_CN.UTF-8',
    LC_MESSAGES: 'zh_CN.UTF-8',
    LANG: 'zh_CN.UTF-8',
  })
  try {
    const exit = await english.command.waitForClose(COMMAND_TIMEOUT_MS.standard)
    expectExited({ ...exit, stderr: english.stderr() })
    const selected = await run(['help', '@tola/address', '--lang', 'en'], root)
    expectExited(selected)
    expect(english.stdout()).toBe(selected.stdout)
    expect(english.stderr()).toBe('')
  } finally {
    await english.command.terminate()
  }

  const chinese = await run(['help', '@tola/address', '--lang', 'zh'], root)
  expectExited(chinese)
  expect(chinese.stdout).toMatch(/\p{Script=Han}/u)
  expect(chinese.stderr).toBe('')
  expect(chinese.stdout).not.toBe(english.stdout())

  const rejected = await run(['help', '@tola/address', '--lang', 'fr'], root)
  expectExited(rejected, 2)
  expect(rejected.stdout).toBe('')
  expect(rejected.stderr).toContain('--lang')
})

test('configuration tables and the overview follow the language', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)

  const table = await run(['help', '[build]', '--lang', 'zh-Hans'], root)
  expectExited(table)
  expect(table.stdout).toMatch(/\p{Script=Han}/u)
  expect(table.stdout).toContain('[build]')
  expect(table.stderr).toBe('')

  const overview = await run(['help', '--lang', 'zh-Hans'], root)
  expectExited(overview)
  expect(overview.stdout).toMatch(/\p{Script=Han}/u)
  expect(overview.stderr).toBe('')
})

test('redirected help keeps color under CLICOLOR_FORCE', async ({ binary, directory: root }) => {
  const run = startCommand(binary, ['help', '@tola/document'], root, { CLICOLOR_FORCE: '1' })
  const exit = await run.command.waitForClose(COMMAND_TIMEOUT_MS.standard)
  expectExited({ ...exit, stderr: run.stderr() })
  expect(run.stdout()).toContain('Import in Typst:')
  expect(run.stdout()).toContain('\u001b[')
  expect(run.stderr()).toBe('')
})

/** Writes the pager wrapper under `root`, returning its path: it shows the documentation, keeps
 *  `pager-received.txt`, and prints `PAGED`. */
async function writePagerWrapper(root: string): Promise<string> {
  const pager = join(root, 'pager.sh')
  await writeFile(
    pager,
    `#!/bin/sh
tee pager-received.txt
printf 'PAGED\\n'
`,
  )
  await chmod(pager, 0o755)
  return pager
}

test('terminal help pages through the configured pager', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  const pager = await writePagerWrapper(root)

  const run = await runOnTerminal(binary, ['help', '@tola/document', '--color', 'never'], root, {
    TERM: 'xterm-256color',
    TOLA_PAGER: pager,
  })

  expectExited(run)
  const output = run.stdout + run.stderr
  expect(output).toContain('Import in Typst:')
  expect(output).toContain('PAGED')
  expect(await readFile(join(root, 'pager-received.txt'), 'utf8')).toContain('Import in Typst:')
})

test('no-pager writes terminal help directly', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  const pager = await writePagerWrapper(root)

  const run = await runOnTerminal(
    binary,
    ['help', '@tola/document', '--color', 'never', '--no-pager'],
    root,
    {
      TERM: 'xterm-256color',
      TOLA_PAGER: pager,
    },
  )

  expectExited(run)
  const output = run.stdout + run.stderr
  expect(output).toContain('Import in Typst:')
  expect(output).not.toContain('PAGED')
  await expect(readFile(join(root, 'pager-received.txt'))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('interactive help prints its page without a terminal', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const plain = await run(['help', '[site]'], root)
  expectExited(plain)
  for (const flag of ['-i', '--interactive']) {
    const fallback = await run(['help', '[site]', flag], root)
    expectExited(fallback)
    expect(fallback.stdout).toBe(plain.stdout)
    expect(fallback.stderr).toBe('')
  }
})

test('interactive help prints the page on a dumb terminal', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const plain = await runOnTerminal(
    binary,
    ['help', '@tola/address', '--no-pager', '--color', 'never'],
    root,
    { TERM: 'xterm-256color' },
  )
  const interactive = await runOnTerminal(
    binary,
    ['help', '@tola/address', '--interactive', '--no-pager', '--color', 'never'],
    root,
    { TERM: 'dumb' },
  )

  expectExited(interactive)
  expect(interactive.stdout).not.toContain('\x1b[?1049h')
  // The fallback writes exactly the page the plain path writes.
  expect(interactive.stdout).toBe(plain.stdout)
})

test('machine-readable help preserves content and targets', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const markdown = await run(['help', 'config', '--format', 'markdown'], root)
  const json = await run(['help', 'config', '--format', 'json'], root)
  expectExited(markdown)
  expectExited(json)
  const document = JSON.parse(json.stdout)
  expect(document.markdown).toBe(markdown.stdout)
  expect(document.target).toBe('tola-help://config')
  expect(document.headings.length).toBeGreaterThan(0)
  const link = document.links.find((link: { target: string }) =>
    link.target.startsWith('tola-help://config/')
  )
  expect(link).toBeDefined()
  expectExited(await run(['help', link.target, '--format', 'json'], root))
})

test('help selects the requested section', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const response = await run(['help', '@tola/address', '--format', 'json'], root)
  expectExited(response)
  const page = JSON.parse(response.stdout)
  const heading = page.headings.find((heading: { level: number }) => heading.level > 1)
  const selected = await run(['help', page.target + '#' + heading.anchor, '--format', 'json'], root)
  expectExited(selected)
  const section = JSON.parse(selected.stdout)
  expect(section.headings[0].anchor).toBe(heading.anchor)
  expect(page.markdown).toContain(section.markdown)
  expectExited(await run(['help', page.target + '#no-such-section', '--format', 'json'], root), 1)
})

test('static help discovers the selected site without running it', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const site = join(root, 'site')
  await mkdir(join(site, 'module'), { recursive: true })
  await writeFile(join(site, 'tola.toml'), "[modules.forest]\npath = 'module'\n")
  await writeFile(
    join(site, 'module/tola-module.toml'),
    `schema-version = 1
[queries.nodes]
entry = 'missing.typ'
label = 'Tree nodes'
schema-version = 1
[commands.edit]
help = 'Edit a tree'
kind = 'argv'
argv = ['missing-tool']
`,
  )
  const config = join(site, 'tola.toml')
  for (
    const [args, name] of [
      [['--config', config, '--help'], 'forest edit'],
      [['query', '-C' + config, '--help'], 'forest.nodes'],
    ] as const
  ) {
    const help = await run([...args], root)
    expectExited(help)
    expect(help.stdout).toContain(name)
  }
  const query = await run(['query', '-C' + config, 'forest.nodes'], root)
  expectExited(query, 1)
  await expect(readFile(join(site, '.tola'))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('static help tolerates broken site configuration', async ({ binary, directory: root }) => {
  await writeFile(join(root, 'tola.toml'), 'not valid = [')
  const help = await commandRunner(binary)(['--help', '--log-file', 'help.log'], root)
  expectExited(help)
  expect(help.stdout).toContain('build')
  expect(await readdir(root)).toEqual(['tola.toml'])
})
