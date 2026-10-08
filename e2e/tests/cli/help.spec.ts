import { expect } from '@playwright/test'
import { chmod, readdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { stripVTControlCharacters } from 'node:util'
import {
  captureOutput,
  COMMAND_TIMEOUT_MS,
  commandRunner,
  expectExited,
  runCommand,
  RunningProcess,
  startCommand,
  test,
} from '../../support/process.ts'
import { runOnTerminal, screenText, sizedTerminalArgs } from '../../support/terminal.ts'

test('documentation help needs no usable site', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeFile(join(root, 'tola.toml'), 'This is not TOML')
  await writeFile(join(root, 'site.typ'), '#panic("Help must not compile this site")')

  const hooks = await run(['help', 'config', 'build.hooks'], root)
  expectExited(hooks)
  for (const stage of ['before-build', 'generate-outputs', 'after-publish']) {
    expect(hooks.stdout).toContain(`[[build.hooks.${stage}]]`)
  }
  expect(hooks.stderr).toBe('')

  const fonts = await run(['help', 'config', 'typst'], root)
  expectExited(fonts)
  expect(fonts.stdout).toContain('[typst.fonts]')
  expect(fonts.stdout).toContain('paths =')
  expect(fonts.stderr).toBe('')

  for (
    const name of [
      'site',
      'address',
      'icon',
      'image',
      'source',
      'collection',
      'schema',
      'document',
      'code',
      'web',
    ]
  ) {
    const reference = await run(['help', 'package', name], root)
    expectExited(reference)
    expect(reference.stdout).toContain(`@tola/${name}:0.0.0`)
    expect(reference.stdout).toContain('Exports:')
    expect(reference.stderr).toBe('')
  }

  const selected = await run(['help', 'package', 'document', 'headings', 'references'], root)
  expectExited(selected)
  expect(selected.stdout).toContain('let headings(')
  expect(selected.stdout).toContain('depth: ')
  expect(selected.stdout).toContain('Named Parameters')
  expect(selected.stdout).toContain('type: ')
  expect(selected.stdout).toContain('references(')
  expect(selected.stdout).not.toContain('current-document — function')

  const rejected = await run(['help', 'package', 'document', 'headings', 'unknown-export'], root)
  expectExited(rejected, 1)
  expect(rejected.stdout).toBe('')
  expect(rejected.stderr).toContain('unknown-export')
  expect(rejected.stderr).toContain('headings')
  expect((await readdir(root)).sort()).toEqual(['site.typ', 'tola.toml'])
})

test('documentation color follows stdout choice', async ({ binary, directory: root }) => {
  for (const target of [['config', 'typst.fonts'], ['package', 'document', 'headings']]) {
    const plain = await runCommand(
      binary,
      ['--color', 'never', 'help', ...target],
      root,
      COMMAND_TIMEOUT_MS.standard,
    )
    const automatic = await runCommand(
      binary,
      ['--color', 'auto', 'help', ...target],
      root,
      COMMAND_TIMEOUT_MS.standard,
    )
    const colored = await runCommand(
      binary,
      ['--color', 'always', 'help', ...target],
      root,
      COMMAND_TIMEOUT_MS.standard,
    )
    for (const command of [plain, automatic, colored]) {
      expectExited(command)
      expect(command.stderr).toBe('')
    }
    expect(plain.stdout).not.toContain('\u001b')
    expect(automatic.stdout).toBe(plain.stdout)
    expect(colored.stdout).toContain('\u001b[')
    expect(stripVTControlCharacters(colored.stdout)).toBe(plain.stdout)
    expect(
      plain.stdout,
    ).toContain(target[0] === 'config' ? 'paths =' : 'let headings(')
  }
})

test('package documentation follows the language', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)

  const english = await run(['help', 'package', 'address'], root)
  expectExited(english)
  expect(english.stdout).toContain('Import in Typst:')
  expect(english.stdout).toContain('Decode a percent-encoded site-root URL path exactly once.')
  expect(english.stderr).toBe('')

  const chinese = await run(['help', 'package', 'address', '--lang', 'zh'], root)
  expectExited(chinese)
  expect(chinese.stdout).toContain('Import in Typst:')
  expect(chinese.stdout).toContain('把百分号编码的站点根 URL 路径恰好解码一次。')
  expect(chinese.stdout).toContain('Positional Parameters')
  expect(chinese.stderr).toBe('')

  // A translation keeps every identifier it reads, so the keys it names stay the keys Tola has.
  const sources = await run(['help', 'package', 'source', '--lang', 'zh'], root)
  expectExited(sources)
  expect(sources.stdout).toContain('build.content-dir')

  const rejected = await run(['help', 'package', 'address', '--lang', 'fr'], root)
  expectExited(rejected, 2)
  expect(rejected.stdout).toBe('')
  expect(rejected.stderr).toContain('--lang')
})

test('configuration tables and the overview follow the language', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)

  const table = await run(['help', 'config', 'build', '--lang', 'zh-Hans'], root)
  expectExited(table)
  // The prose is Chinese; the key names and the headings stay English.
  expect(table.stdout).toMatch(/\p{Script=Han}/u)
  expect(table.stdout).toContain('[build]')
  expect(table.stdout).toContain('content-dir')
  expect(table.stdout).toContain('publish-dir')
  expect(table.stderr).toBe('')

  const overview = await run(['help', '--lang', 'zh-Hans'], root)
  expectExited(overview)
  expect(overview.stdout).toMatch(/\p{Script=Han}/u)
  for (const target of ['config', 'package']) {
    expect(overview.stdout).toContain(`tola help ${target}`)
  }
  expect(overview.stderr).toBe('')
})

test('redirected help keeps color under CLICOLOR_FORCE', async ({ binary, directory: root }) => {
  const run = startCommand(binary, ['help', 'package', 'document'], root, { CLICOLOR_FORCE: '1' })
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

  const run = await runOnTerminal(binary, ['help', 'package', 'document', '--color', 'never'], root, {
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
    ['help', 'package', 'document', '--color', 'never', '--no-pager'],
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
  const plain = await run(['help', 'config', 'site'], root)
  expectExited(plain)
  for (const flag of ['-i', '--interactive']) {
    const fallback = await run(['help', 'config', 'site', flag], root)
    expectExited(fallback)
    expect(fallback.stdout).toBe(plain.stdout)
    expect(fallback.stderr).toBe('')
  }
})

test('interactive help prints the page on a dumb terminal', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const plain = await runOnTerminal(
    binary,
    ['help', 'package', 'address', '--no-pager', '--color', 'never'],
    root,
    { TERM: 'xterm-256color' },
  )
  const interactive = await runOnTerminal(
    binary,
    ['help', 'package', 'address', '--interactive', '--no-pager', '--color', 'never'],
    root,
    { TERM: 'dumb' },
  )

  expectExited(interactive)
  expect(interactive.stdout).not.toContain('\x1b[?1049h')
  // The fallback writes exactly the page the plain path writes.
  expect(interactive.stdout).toBe(plain.stdout)
})

test('interactive help enters, scrolls, and quits', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, ['help', 'config', 'site', '--interactive', '--color', 'never']),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)

  await expect.poll(() => screenText(output.stdout())).toContain('configuration')
  expect(output.stdout()).toContain('\x1b[?1049h')
  // The end of the page is only drawn once the reader goes there.
  expect(screenText(output.stdout())).not.toContain('values,')
  running.child.stdin.write('G')
  await expect.poll(() => screenText(output.stdout())).toContain('values,')
  running.child.stdin.write('q')

  expect((await closing).code).toBe(0)
  expect(output.stdout()).toContain('\x1b[?1049l')
})

test('interactive help cancels on interrupt', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, ['help', 'config', 'site', '--interactive', '--color', 'never']),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)

  await expect.poll(() => stripVTControlCharacters(output.stdout())).toContain('configuration')
  running.child.stdin.write('\x03')

  expect((await closing).code).toBe(130)
  expect(output.stdout()).toContain('\x1b[?1049l')
})

test('interactive help keeps the zh page intact', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, [
      'help',
      'package',
      'address',
      '--lang',
      'zh',
      '--interactive',
      '--color',
      'never',
    ]),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)

  // Every character of the page's overview reaches the drawn rows, in order: jumps between
  // wide glyphs must not leave any behind.
  const sentence = 'slug、route、输出路径与浏览器'
  await expect
    .poll(() => {
      const drawn = stripVTControlCharacters(output.stdout()).replace(/\s+/g, '')
      let position = 0
      for (const character of sentence) {
        position = drawn.indexOf(character, position) + 1
        if (position === 0) return false
      }
      return true
    })
    .toBe(true)

  running.child.stdin.write('G')
  await expect.poll(() => screenText(output.stdout())).toContain('100%/index.html')
  running.child.stdin.write('g')
  await expect.poll(() => screenText(output.stdout())).toContain('Exports:')
  running.child.stdin.write('\t')
  await expect.poll(() => screenText(output.stdout())).toContain('Esc/Tab')
  running.child.stdin.write('\t')
  await expect.poll(() => screenText(output.stdout())).not.toContain('Esc/Tab')
  running.child.stdin.write('q')

  expect((await closing).code).toBe(0)
  expect(output.stdout()).toContain('\x1b[?1049l')
})

test('pasted help search reveals matches before Enter', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, ['help', 'package', 'address', '--interactive', '--color', 'never']),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)
  const content = () => screenText(output.stdout()).split('\n').slice(0, -2).join('\n')

  await expect.poll(content).toContain('Import in Typst:')
  expect(content()).not.toContain('100%/index.html')
  running.child.stdin.write('/\x1b[200~100%/index.html\x1b[201~')
  await expect.poll(content).toContain('100%/index.html')
  running.child.stdin.write('\rq')

  expect((await closing).code).toBe(0)
  expect(output.stdout()).toContain('\x1b[?1049l')
})

test('help section keys align export headings', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, [
      'help',
      'package',
      'address',
      'route',
      'decode-url-path',
      '--interactive',
      '--color',
      'never',
    ]),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)
  const firstLine = () => screenText(output.stdout()).split('\n')[0]

  await expect.poll(firstLine).toContain('@tola/address')
  running.child.stdin.write('f')
  await expect.poll(firstLine).toMatch(/^route\b/)
  running.child.stdin.write(' ')
  await expect.poll(firstLine).not.toMatch(/^route\b/)
  running.child.stdin.write('b')
  await expect.poll(firstLine).toMatch(/^route\b/)
  running.child.stdin.write('f')
  await expect.poll(firstLine).toMatch(/^decode-url-path\b/)
  running.child.stdin.write('b')
  await expect.poll(firstLine).toMatch(/^route\b/)
  running.child.stdin.write('q')

  expect((await closing).code).toBe(0)
})
