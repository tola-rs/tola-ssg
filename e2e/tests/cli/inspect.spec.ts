import { expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import {
  COMMAND_TIMEOUT_MS,
  captureOutput,
  commandRunner,
  expectExited,
  RunningProcess,
  startCommand,
  test,
} from '../../support/process.ts'
import { hookToml, writeHookConfiguration } from '../../support/hooks.ts'
import { writeMinimalSite } from '../../support/site.ts'
import { proxyEnvironment, startRefusingProxy } from '../../support/proxy.ts'
import { screenText, sizedTerminalArgs } from '../../support/terminal.ts'

test('inspect sources discovers files regardless of metadata', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const inspectSources = async (args: string[]) => {
    const result = await run(['inspect', 'sources', ...args], root, COMMAND_TIMEOUT_MS.standard)
    expectExited(result)
    return result.stdout
  }
  await writeMinimalSite(root)
  const source = join(root, 'content/index.typ')
  const destination = join(root, 'sources.json')
  await writeFile(source, '#import "@tola/source:0.0.0": tola-meta\n#tola-meta((title: "Published"))\n')
  await inspectSources(['--output', destination])
  expect(JSON.parse(await readFile(destination, 'utf8'))).toEqual([
    { path: 'content/index.typ', contentPath: 'index.typ' },
  ])

  await writeFile(source, 'No source metadata.\n')
  await inspectSources(['--output', destination])
  const rows = [{ path: 'content/index.typ', contentPath: 'index.typ' }]
  expect(JSON.parse(await readFile(destination, 'utf8'))).toEqual(rows)
  expect(JSON.parse(await inspectSources([]))).toEqual(rows)
  await expect(readFile(join(root, 'public/index.html'))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('check reports the candidate without running hooks or publishing', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const hook = join(root, 'hook.cjs')
  await writeMinimalSite(root)
  await writeFile(hook, "require('node:fs').writeFileSync('hook-ran.txt', 'ran');\n")
  await writeHookConfiguration(root, [hookToml('before-build', hook, { name: 'hook' })])

  const result = await run(['check', '--no-hooks', '--json'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(result)
  const report = JSON.parse(result.stdout)
  expect(report.published).toBe(false)
  expect(report.input.hookExecution).toBe('skipped')
  expect(report.pages).toEqual([expect.objectContaining({ output: 'index.html' })])
  await expect(readFile(join(root, 'hook-ran.txt'))).rejects.toMatchObject({ code: 'ENOENT' })
  await expect(readFile(join(root, 'public/index.html'))).rejects.toMatchObject({ code: 'ENOENT' })
})

for (const scope of ['--offline', '--pure']) {
  test(`${scope} inspection refuses icon downloads`, async ({ binary, directory: root }) => {
    await writeMinimalSite(root)
    await writeFile(
      join(root, 'content/index.typ'),
      '#import "@tola/source:0.0.0": tola-meta\n#tola-meta((title: "Local source"))\n',
    )
    await writeFile(
      join(root, 'tola.toml'),
      '[icons.collections.lucide]\nsource-type = "remote-json"\npreset = "lucide"\n',
    )
    const proxy = await startRefusingProxy()
    try {
      for (
        const [args, code] of [
          [['inspect', 'config'], 0],
          [['inspect', 'sources'], 0],
          [['inspect', 'icons'], 1],
        ] as const
      ) {
        const command = startCommand(binary, [scope, ...args], root, proxyEnvironment(proxy.url))
        try {
          const completed = await command.command.waitForClose(COMMAND_TIMEOUT_MS.standard)
          expectExited({ ...completed, stderr: command.stderr() }, code)
        } finally {
          await command.command.terminate()
        }
      }
      expect(proxy.requests).toEqual([])
    } finally {
      await proxy.close()
    }
  })
}

test('retired declaration reports once', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  const source = join(root, 'content/index.typ')
  await writeFile(source, '#metadata((title: "Retired")) <tola-meta>\n')

  const retired = await run(['check'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(retired)
  expect(retired.stderr.match(/source\.declaration_deprecated/g)).toHaveLength(1)
  expect(retired.stderr).toContain('`content/index.typ`')

  await writeFile(source, '#import "@tola/source:0.0.0": tola-meta\n#tola-meta((title: "Declared"))\n')
  const declared = await run(['check'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(declared)
  expect(declared.stderr).not.toContain('source.declaration_deprecated')
})

test('redirected inspection preserves discovered source rows', async ({ binary, directory: root }) => {
  await writeMinimalSite(root)
  await writeFile(
    join(root, 'content/index.typ'),
    '#let = invalid syntax\n',
  )
  const run = commandRunner(binary)
  const args = ['inspect', 'sources']
  const ordinary = await run(args, root)
  const interactive = await run([...args, '--interactive'], root)
  expectExited(ordinary)
  expectExited(interactive)
  const rows = [{ path: 'content/index.typ', contentPath: 'index.typ' }]
  expect(JSON.parse(interactive.stdout)).toEqual(rows)
  expect(JSON.parse(ordinary.stdout)).toEqual(rows)
  const exported = await run([...args, '--interactive', '--output', 'rows.json'], root)
  expectExited(exported)
  expect(exported.stdout).toBe('')
  expect(JSON.parse(await readFile(join(root, 'rows.json'), 'utf8'))).toEqual(rows)
})

test('inspection selects the original row from filtered details', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await writeFile(
    join(root, 'select.nu'),
    `use tola ui
let rows = [{path: 'a.typ', title: Alpha}, {path: 'b.typ', title: Beta, draft: false, raw: {func: text, text: hello}}]
let selected = $rows | ui choose --title 'Filtered selection'
let empty = [] | ui choose
let null_choice = [null] | ui choose --title 'Null selection'
{selected: $selected, empty: $empty, null_choice: $null_choice} | to json --raw | save --raw 'selection.json'
`,
  )
  const terminal = new RunningProcess('script', sizedTerminalArgs(binary, ['nu', 'select.nu']), root, {
    TERM: 'xterm-256color',
  })
  const captured = captureOutput(terminal.child)
  const screen = () => screenText(captured.stdout())
  try {
    await expect.poll(screen).toContain('Filtered selection')
    terminal.child.stdin.write('/Beta')
    await expect.poll(() => screen().includes('Beta') && !screen().includes('Alpha')).toBe(true)
    terminal.child.stdin.write('\t\t\x1b[C\x1b[B\r')
    await expect.poll(screen).toContain('Null selection')
    terminal.child.stdin.write('\r')
    const exit = await terminal.waitForClose(COMMAND_TIMEOUT_MS.standard)
    expectExited({ ...exit, stderr: captured.stderr() })
    expect(JSON.parse(await readFile(join(root, 'selection.json'), 'utf8'))).toEqual({
      selected: {
        action: 'submit',
        selected: { path: 'b.typ', title: 'Beta', draft: false, raw: { func: 'text', text: 'hello' } },
      },
      empty: { action: 'quit' },
      null_choice: { action: 'submit', selected: null },
    })
  } finally {
    await terminal.terminate()
  }
})
