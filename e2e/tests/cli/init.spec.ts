import { expect } from '@playwright/test'
import { mkdir, readdir, readFile, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { stripVTControlCharacters } from 'node:util'
import { readLogRecords } from '../../support/log.ts'
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
import { screenText, sizedTerminalArgs } from '../../support/terminal.ts'

test('init writes one scaffold everywhere', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  const scaffolds: { configuration: string; program: string }[] = []
  for (const directory of ['current', 'dot', 'named']) {
    const root = join(temporary, directory)
    await mkdir(root)
    const args = directory === 'current' ? [] : directory === 'dot' ? ['.'] : [directory]
    const cwd = directory === 'named' ? temporary : root
    const completed = await runInit(['init', ...args], cwd, COMMAND_TIMEOUT_MS.short)

    expectExited(completed)
    expect(completed.stdout).toBe('')
    expect(await readdir(join(root, 'content'))).toEqual([])
    scaffolds.push({
      configuration: await readFile(join(root, 'tola.toml'), 'utf8'),
      program: await readFile(join(root, 'site.typ'), 'utf8'),
    })
  }
  expect(scaffolds[1]).toEqual(scaffolds[0])
  expect(scaffolds[2]).toEqual(scaffolds[0])
})

test('pure init builds with embedded packages', async ({ binary, directory: temporary }) => {
  const root = join(temporary, 'site')
  const initialized = startCommand(binary, ['init', root, '--pure'], temporary, {
    TYPST_PACKAGE_PATH: 'relative-host-packages',
    TYPST_PACKAGE_CACHE_PATH: 'relative-host-cache',
  })
  try {
    const completed = await initialized.command.waitForClose(COMMAND_TIMEOUT_MS.short)
    expectExited({ ...completed, stderr: initialized.stderr() })
  } finally {
    await initialized.command.terminate()
  }
  const built = await runCommand(binary, ['build', '--offline', '--pure'], root, COMMAND_TIMEOUT_MS.build)
  expectExited(built)
  expect(await readFile(join(root, 'public/404.html'), 'utf8')).toContain('Page not found')
})

test('internal log records the scaffold', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  const root = join(await realpath(temporary), "Ada's notes")
  const log = join(root, '.tola/logs/init.jsonl')
  const completed = await runInit(
    ['init', root, '--log-file', log],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed)
  expect(completed.stdout).toBe('')
  const directoryStep = completed.stderr.indexOf(`Switch to \`${root}\`.`)
  expect(directoryStep).toBeGreaterThan(-1)
  const contentStep = completed.stderr.indexOf('content/index.typ', directoryStep)
  const developmentStep = completed.stderr.indexOf('tola dev', contentStep)
  expect(contentStep).toBeGreaterThan(directoryStep)
  expect(developmentStep).toBeGreaterThan(contentStep)
  expect(completed.stderr).toContain('tola help "[site]"')
  expect(completed.stderr).toContain('tola help "@tola/address" slugify output-to-url')
  expect(await readdir(join(root, 'content'))).toEqual([])
  expect(await readFile(join(root, 'tola.toml'), 'utf8')).toContain('[build]')
  const records = await readLogRecords(log)
  expect(records[0]!.fields).toMatchObject({ kind: 'command_started', command: 'init' })
  expect(records.at(-1)!.fields).toMatchObject({ kind: 'command_finished', success: true })
})

test('conflicting init keeps existing files', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  const root = join(temporary, 'site')
  const log = join(temporary, 'init.jsonl')
  await mkdir(root)
  await writeFile(join(root, 'site.typ'), 'Existing site program.\n')
  await writeFile(join(root, 'keep.txt'), 'Keep this file.\n')
  const completed = await runInit(
    ['init', root, '--force', '--log-file', log],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed, 1)
  expect(completed.stdout).toBe('')
  expect(completed.stderr).toContain('site.typ')
  expect((await readdir(root)).sort()).toEqual(['keep.txt', 'site.typ'])
  expect(await readFile(join(root, 'site.typ'), 'utf8')).toBe('Existing site program.\n')
  expect(await readFile(join(root, 'keep.txt'), 'utf8')).toBe('Keep this file.\n')
  const records = await readLogRecords(log)
  expect(records.some((record) =>
    record.fields?.kind === 'diagnostic' &&
    record.fields.code === 'init.conflict'
  )).toBe(true)
  expect(records.at(-1)!.fields).toMatchObject({ kind: 'command_finished', success: false })
})

test('dry-run writes no site files', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  await writeFile(join(temporary, 'site.typ'), 'Keep the existing program.\n')
  const completed = await runInit(
    [
      'init',
      '.',
      '--dry-run',
      '--log-file',
      '.tola/logs/init.jsonl',
    ],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed, 1)
  expect(completed.stdout).toContain('[build]')
  expect(completed.stdout).toContain('entry = "site.typ"')
  expect(completed.stdout).toContain('[site]')
  expect(completed.stdout).not.toContain('error[')
  expect(completed.stderr).toContain('error[init.conflict]')
  expect(completed.stderr).not.toContain('[build]')
  expect(await readdir(temporary)).toEqual(['site.typ'])
  expect(await readFile(join(temporary, 'site.typ'), 'utf8')).toBe('Keep the existing program.\n')
})

test('init survives a small descriptor budget', async ({ binary, directory: temporary }) => {
  test.skip(process.platform === 'win32', 'ulimit is a POSIX shell builtin')
  const root = join(temporary, 'limited')
  // The scaffold materializes every embedded package, so a writer that holds one
  // directory handle per file exhausts the default limit on macOS and Linux.
  const completed = await runCommand(
    '/bin/sh',
    ['-c', `ulimit -n 32; exec '${binary}' init '${root}' --color never`],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed)
  expect(await readFile(join(root, 'tola.toml'), 'utf8')).toContain('entry = "site.typ"')
  expect(await readFile(join(root, 'site.typ'), 'utf8')).toContain('#let pages')
})

test('features match the medium preset', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  const preset = join(temporary, 'preset')
  const features = join(temporary, 'features')
  expectExited(
    await runInit(
      ['init', preset, '--preset', 'medium', '--pure'],
      temporary,
      COMMAND_TIMEOUT_MS.standard,
    ),
  )
  expectExited(
    await runInit(
      ['init', features, '--features', 'starter-stylesheet,feed,sitemap', '--pure'],
      temporary,
      COMMAND_TIMEOUT_MS.standard,
    ),
  )

  const scaffold = async (root: string) => ({
    listing: (await readdir(root, { recursive: true })).sort(),
    configuration: await readFile(join(root, 'tola.toml'), 'utf8'),
    program: await readFile(join(root, 'site.typ'), 'utf8'),
    page: await readFile(join(root, 'site/page.typ'), 'utf8'),
    stylesheet: await readFile(join(root, 'static/web-assets/css/site.css'), 'utf8'),
  })
  expect(await scaffold(features)).toEqual(await scaffold(preset))
})

test('feature selection builds offline', async ({ binary, directory: temporary }) => {
  const root = join(temporary, 'site')
  const runInit = commandRunner(binary)
  expectExited(
    await runInit(['init', root, '--features', 'feed', '--pure'], temporary, COMMAND_TIMEOUT_MS.standard),
  )

  const built = await runCommand(binary, ['build', '--offline', '--pure'], root, COMMAND_TIMEOUT_MS.build)
  expectExited(built)
  expect(await readFile(join(root, 'public/404.html'), 'utf8')).toContain('Page not found')
  expect(await readdir(join(root, 'public'))).not.toContain('feed.xml')
})

test('feature selection without a runner reports a diagnostic', async ({ binary, directory: temporary }) => {
  const root = join(temporary, 'site')
  const runInit = commandRunner(binary)
  const completed = await runInit(
    ['init', root, '--features', 'tailwind-css', '--pure'],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed, 1)
  expect(completed.stderr).toContain('error[init.selection]')
  expect(completed.stderr).toContain('add `deno-toolchain` to `--features`')
  expect(await readdir(temporary)).toEqual([])
})

test('unknown feature names are refused', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  const completed = await runInit(
    ['init', 'site', '--features', 'feeds', '--pure'],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed, 2)
  expect(completed.stderr).toContain('--features <NAMES>')
  expect(completed.stderr).toContain('deno-toolchain')
  expect(await readdir(temporary)).toEqual([])
})

test('feature dry run writes nothing', async ({ binary, directory: temporary }) => {
  const runInit = commandRunner(binary)
  const completed = await runInit(
    ['init', 'site', '--features', 'starter-stylesheet', '--dry-run', '--pure'],
    temporary,
    COMMAND_TIMEOUT_MS.short,
  )

  expectExited(completed)
  expect(completed.stdout).toContain('entry = "site.typ"')
  expect(await readdir(temporary)).toEqual([])
})

type InteractiveInit = {
  site: string
  running: RunningProcess
  output: ReturnType<typeof captureOutput>
}

async function withInteractiveInit(
  binary: string,
  root: string,
  args: string[],
  interact: (init: InteractiveInit) => Promise<void>,
  term = 'xterm-256color',
): Promise<void> {
  const site = join(root, 'site')
  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, [
      'init',
      site,
      '--interactive',
      '--editor',
      'vscode',
      '--pure',
      '--color',
      'never',
      ...args,
    ]),
    root,
    { TERM: term },
  )
  const output = captureOutput(running.child)
  try {
    await interact({ site, running, output })
  } finally {
    await running.terminate()
  }
}

async function confirmInteractiveInit(init: InteractiveInit, screen = true): Promise<void> {
  if (screen) init.running.child.stdin.write('y')
  await expect.poll(() => stripVTControlCharacters(init.output.stdout())).toContain('Create this site?')
  init.running.child.stdin.write('y\r')
  expect((await init.running.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(0)
}

async function filterInit(init: InteractiveInit, query: string, previous = ''): Promise<void> {
  init.running.child.stdin.write('/')
  init.running.child.stdin.write('\x7f'.repeat(previous.length) + query)
  await expect.poll(() => screenText(init.output.stdout())).toContain(`filter: ${query}`)
  init.running.child.stdin.write('\r')
  await expect.poll(() => screenText(init.output.stdout())).toContain(`/${query}`)
}

test('interactive init writes the selected preset', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await withInteractiveInit(binary, root, [], async (init) => {
    await expect.poll(() => screenText(init.output.stdout())).toContain('preset  [1] rich')
    await expect.poll(() => screenText(init.output.stdout())).toContain('[■] feed')
    init.running.child.stdin.write('3')
    await expect.poll(() => screenText(init.output.stdout())).toContain('[ ] feed')
    await confirmInteractiveInit(init)
    expect(stripVTControlCharacters(init.output.stdout())).toContain('Features: minimal')
    expect(await readdir(join(init.site, 'static/web-assets/css'))).toEqual([])
    expect(await readdir(init.site)).not.toContain('justfile')
  })
})

test('interactive file selection uses its available writers', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await withInteractiveInit(binary, root, ['--preset', 'minimal'], async (init) => {
    await expect.poll(() => screenText(init.output.stdout())).toContain('preset  [1] rich')
    await filterInit(init, 'justfile')
    init.running.child.stdin.write(' ')
    await expect.poll(() => screenText(init.output.stdout())).toContain(
      'check pagefind or deno-toolchain or tailwind-css',
    )
    await filterInit(init, 'static/web-assets/css/site.css', 'justfile')
    init.running.child.stdin.write(' ')
    await expect.poll(() => screenText(init.output.stdout())).toContain('[■] static/web-assets/css/site.css')
    await confirmInteractiveInit(init)
    expect(await readdir(join(init.site, 'static/web-assets/css'))).toEqual(['site.css'])
    expect(await readdir(init.site)).not.toContain('justfile')
  })
})

test('interactive feature selection completes the toolchain', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await withInteractiveInit(binary, root, ['--preset', 'minimal'], async (init) => {
    await expect.poll(() => screenText(init.output.stdout())).toContain('preset  [1] rich')
    await filterInit(init, 'tailwind-css')
    init.running.child.stdin.write(' ')
    await expect.poll(() => screenText(init.output.stdout())).toContain('[■] tailwind-css')
    await filterInit(init, 'deno-toolchain', 'tailwind-css')
    await expect.poll(() => screenText(init.output.stdout())).toContain('[■] deno-toolchain')
    await confirmInteractiveInit(init)
    const configuration = JSON.parse(await readFile(join(init.site, 'deno.json'), 'utf8'))
    expect(configuration.tasks.css).toContain('tailwindcss')
    expect(await readFile(join(init.site, 'justfile'), 'utf8')).toContain('deno task css')
    const stylesheet = await readFile(join(init.site, 'static/tailwind-sources/site.css'), 'utf8')
    expect(stylesheet).toContain('tailwindcss/utilities.css')
    expect(stylesheet).toContain('color-scheme: light dark')
    expect(stylesheet).not.toContain('{{stylesheet}}')
  })
})

for (const term of ['xterm-256color', 'dumb']) {
  test(`interactive seed normalizes stylesheet providers on ${term}`, async ({ binary, directory: root }) => {
    test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
    await withInteractiveInit(
      binary,
      root,
      ['--features', 'starter-stylesheet,tailwind-css'],
      async (init) => {
        if (term !== 'dumb') {
          await expect.poll(() => screenText(init.output.stdout())).toContain('[ ] starter-stylesheet')
          await expect.poll(() => screenText(init.output.stdout())).toContain('[■] deno-toolchain')
          await expect.poll(() => screenText(init.output.stdout())).toContain('[■] tailwind-css')
        }
        await confirmInteractiveInit(init, term !== 'dumb')
        expect(await readdir(join(init.site, 'static/web-assets/css'))).toEqual([])
        expect(await readdir(init.site)).toContain('deno.json')
        expect(await readFile(join(init.site, 'static/tailwind-sources/site.css'), 'utf8')).not.toContain(
          '{{stylesheet}}',
        )
        if (term === 'dumb') expect(init.output.stdout()).not.toContain('\x1b[?1049h')
      },
      term,
    )
  })
}

test('interactive init cancels on interrupt', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await withInteractiveInit(binary, root, [], async (init) => {
    await expect.poll(() => screenText(init.output.stdout())).toContain('preset  [1] rich')
    init.running.child.stdin.write('\x03')
    expect((await init.running.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(130)
    expect(init.output.stdout()).toContain('\x1b[?1049l')
    expect(await readdir(root)).toEqual([])
  })
})

test('interactive init falls back to the preset question', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await withInteractiveInit(binary, root, ['--preset', 'minimal'], async (init) => {
    await expect.poll(() => stripVTControlCharacters(init.output.stdout())).toContain(
      'Choose a scaffold preset',
    )
    init.running.child.stdin.write('\r')
    await confirmInteractiveInit(init, false)
    expect(init.output.stdout()).not.toContain('\x1b[?1049h')
    expect(await readdir(init.site)).not.toContain('justfile')
  }, 'dumb')
})
