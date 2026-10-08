import { expect } from '@playwright/test'
import { mkdir, readdir, readFile, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { COMMAND_TIMEOUT_MS, commandRunner, expectExited, startCommand, test } from '../../support/process.ts'
import { previewDemo } from '../../support/server.ts'

test('demo documentation and source need no site', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  await writeFile(join(directory, 'tola.toml'), 'not a valid site')
  const guide = await run(['help', 'demo', 'backlinks'], directory)
  expectExited(guide)
  expect(guide.stdout).toContain('site/page.typ')
  const address = await run(['help', 'tola-help://demos/backlinks'], directory)
  expectExited(address)
  expect(address.stdout).toBe(guide.stdout)
  const source = await run(['help', 'demo', 'backlinks', 'site/page.typ'], directory)
  expectExited(source)
  expect(source.stdout).toContain('#let page(')
  expect(await readdir(directory)).toEqual(['tola.toml'])
})

test('demo export keeps the displayed source executable', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  const destination = join(directory, 'exported site')
  const exported = await run(['help', 'demo', 'backlinks', '--export', destination], directory)
  expectExited(exported)
  const shown = await run(['help', 'demo', 'backlinks', 'site/page.typ'], directory)
  expectExited(shown)
  expect(await readFile(join(destination, 'site/page.typ'), 'utf8')).toBe(shown.stdout)
  const checked = await run(['--pure', 'check'], destination, COMMAND_TIMEOUT_MS.build)
  expectExited(checked)

  const changed = join(destination, 'site.typ')
  await writeFile(changed, 'author changes')
  const refused = await run(['help', 'demo', 'backlinks', '--export', destination], directory)
  expectExited(refused, 1)
  expect(await readFile(changed, 'utf8')).toBe('author changes')
})

test('empty export destinations stay empty', async ({ binary, directory }) => {
  const destination = join(directory, 'existing')
  await mkdir(destination)
  const refused = await commandRunner(binary)(
    ['help', 'demo', 'backlinks', '--export', destination],
    directory,
  )
  expectExited(refused, 1)
  expect(await readdir(destination)).toEqual([])
})

test('export logs cannot create the destination', async ({ binary, directory }) => {
  const destination = join(directory, 'export')
  const refused = await commandRunner(binary)([
    '--log-file',
    join(destination, 'session.jsonl'),
    'help',
    'demo',
    'backlinks',
    '--export',
    destination,
  ], directory)
  expectExited(refused, 1)
  await expect(readdir(destination)).rejects.toMatchObject({ code: 'ENOENT' })
})

test('editor commands receive the persistent export as one argument', async ({ binary, directory }) => {
  const editor = join(directory, 'record editor.ts')
  const recorded = join(directory, 'editor-arguments.json')
  const destination = join(directory, 'editable site')
  await writeFile(
    editor,
    'await Deno.writeTextFile(Deno.env.get("EDITOR_RECORD")!, JSON.stringify(Deno.args))\n',
  )
  const running = startCommand(
    binary,
    [
      'help',
      'demo',
      'backlinks',
      '--export',
      destination,
      '--edit',
      '--color',
      'never',
    ],
    directory,
    {
      TOLA_EDITOR: `"${Deno.execPath()}" run --allow-env --allow-write "${editor}" --wait`,
      EDITOR_RECORD: recorded,
    },
  )
  try {
    const exit = await running.command.waitForClose(COMMAND_TIMEOUT_MS.build)
    expectExited({ ...exit, stderr: running.stderr() })
    const argumentsReceived = JSON.parse(await readFile(recorded, 'utf8')) as string[]
    expect(argumentsReceived).toHaveLength(2)
    expect(argumentsReceived[0]).toBe('--wait')
    expect(await realpath(argumentsReceived[1]!)).toBe(await realpath(destination))
    expect(await readFile(join(destination, 'tola.toml'), 'utf8')).toContain('[site]')
  } finally {
    await running.command.terminate()
  }
})

test('cancelling an editor retains the exported site', async ({ binary, directory }) => {
  test.skip(process.platform === 'win32', 'Process-group interrupts use the Unix terminal contract')
  const editor = join(directory, 'waiting-editor.ts')
  const destination = join(directory, 'editable-site')
  await writeFile(
    editor,
    [
      'Deno.addSignalListener("SIGINT", () => {})',
      'console.log(`editor-ready:${Deno.pid}`)',
      'setInterval(() => {}, 60_000)',
    ].join('\n'),
  )
  const running = startCommand(
    binary,
    [
      'help',
      'demo',
      'backlinks',
      '--export',
      destination,
      '--edit',
      '--color',
      'never',
    ],
    directory,
    { TOLA_EDITOR: `"${Deno.execPath()}" run "${editor}"` },
  )
  try {
    await expect.poll(running.stdout).toMatch(/editor-ready:\d+/)
    const editorPid = Number(running.stdout().match(/editor-ready:(\d+)/)![1])
    running.command.interrupt()
    const exit = await running.command.waitForClose(COMMAND_TIMEOUT_MS.short)
    expectExited({ ...exit, stderr: running.stderr() }, 130)
    expect(() => process.kill(editorPid, 0)).toThrow()
    expect(await readFile(join(destination, 'tola.toml'), 'utf8')).toContain('[site]')
  } finally {
    await running.command.terminate()
  }
})

test('demo preview ignores the caller site and releases its port', async ({ binary, directory }) => {
  await writeFile(join(directory, 'tola.toml'), 'not a valid site')
  const preview = await previewDemo(binary, 'backlinks', directory)
  try {
    const response = await fetch(preview.url)
    expect(response.ok).toBe(true)
    expect(response.headers.get('content-type')).toContain('text/html')
    expect(await response.text()).toContain('<main>')
    expect(await readdir(directory)).toEqual(['tola.toml'])
  } finally {
    await preview.close()
  }
  await expect(fetch(preview.url)).rejects.toThrow()
})
