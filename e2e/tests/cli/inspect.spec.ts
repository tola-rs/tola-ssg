import { expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import {
  captureOutput,
  COMMAND_TIMEOUT_MS,
  commandRunner,
  expectExited,
  RunningProcess,
  startCommand,
  test,
} from '../../support/process.ts'
import { runOnTerminal, screenText, sizedTerminalArgs } from '../../support/terminal.ts'
import { hookToml, writeHookConfiguration } from '../../support/hooks.ts'
import { writeMinimalSite } from '../../support/site.ts'
import { proxyEnvironment, startRefusingProxy } from '../../support/proxy.ts'

test('inspect sources clears stale metadata', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const inspectSources = async (args: string[]) => {
    const result = await run(['inspect', 'sources', ...args], root, COMMAND_TIMEOUT_MS.standard)
    expectExited(result)
    return result.stdout
  }
  await writeMinimalSite(root)
  const source = join(root, 'content/index.typ')
  const destination = join(root, 'metadata.json')
  await writeFile(source, '#import "@tola/source:0.0.0": tola-meta\n#tola-meta((title: "Published"))\n')
  await inspectSources(['--output', destination])
  expect(JSON.parse(await readFile(destination, 'utf8'))).toEqual([
    { path: 'content/index.typ', title: 'Published' },
  ])

  await writeFile(source, 'No source metadata.\n')
  await inspectSources(['--output', destination])
  expect(JSON.parse(await readFile(destination, 'utf8'))).toEqual([])
  expect(JSON.parse(await inspectSources([]))).toEqual([])

  const emptyDestination = join(root, 'empty.json')
  await inspectSources(['--raw', '--pretty', '--output', emptyDestination])
  expect(JSON.parse(await readFile(emptyDestination, 'utf8'))).toEqual([])
})

test('inspect documents runs no hooks', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const hook = join(root, 'hook.cjs')
  await writeMinimalSite(root)
  await writeFile(hook, "require('node:fs').writeFileSync('hook-ran.txt', 'ran');\n")
  await writeHookConfiguration(root, [hookToml('before-build', hook, { name: 'hook' })])

  const result = await run(['inspect', 'documents'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(result)
  expect(JSON.parse(result.stdout)).toEqual([
    expect.objectContaining({ output: 'index.html' }),
  ])
  await expect(readFile(join(root, 'hook-ran.txt'))).rejects.toMatchObject({ code: 'ENOENT' })
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
          [['config'], 0],
          [['inspect', 'sources'], 1],
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

test('retired declaration registers no metadata', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  const source = join(root, 'content/index.typ')
  await writeFile(source, '#metadata((title: "Retired")) <tola-meta>\n')

  const inspected = await run(['inspect', 'sources'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(inspected)
  expect(inspected.stderr).toContain('0 with metadata')
  expect(JSON.parse(inspected.stdout)).toEqual([])
})

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

/** Writes `count` content sources, each declaring a numbered title. */
async function writeContentSources(root: string, count: number): Promise<void> {
  for (let index = 0; index < count; index += 1) {
    const name = `doc-${String(index).padStart(2, '0')}.typ`
    await writeFile(
      join(root, 'content', name),
      `#import "@tola/source:0.0.0": tola-meta\n#tola-meta((title: "Doc ${index}"))\n`,
    )
  }
}

test('interactive inspection refuses a piped terminal', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  await writeContentSources(root, 1)

  const refused = await run(['inspect', 'sources', '--interactive'], root, COMMAND_TIMEOUT_MS.standard)

  expect(refused.code).toBe(1)
  expect(refused.stderr).toContain('need a terminal')
  expect(refused.stderr).toContain('without the interactive flag')
  expect(refused.stdout).not.toContain('content/doc-00.typ')
})

test('interactive inspection prints the rows on a dumb terminal', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await writeMinimalSite(root)
  await writeContentSources(root, 1)

  const plain = await runOnTerminal(
    binary,
    ['inspect', 'sources', '--color', 'never'],
    root,
    { TERM: 'xterm-256color' },
  )
  const interactive = await runOnTerminal(
    binary,
    ['inspect', 'sources', '--interactive', '--color', 'never'],
    root,
    { TERM: 'dumb' },
  )

  expectExited(interactive)
  expect(interactive.stdout).not.toContain('\x1b[?1049h')
  expect(interactive.stdout).toBe(plain.stdout)
})

test('interactive inspection enters, scrolls, and quits', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await writeMinimalSite(root)
  await writeContentSources(root, 30)

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, ['inspect', 'sources', '--interactive', '--color', 'never']),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)

  await expect.poll(() => screenText(output.stdout())).toContain('content/doc-00.typ')
  expect(output.stdout()).toContain('\x1b[?1049h')
  // The table draws the rows the window holds; the last row is only drawn once the reader
  // goes there.
  expect(screenText(output.stdout())).not.toContain('content/doc-29.typ')
  running.child.stdin.write('G')
  await expect.poll(() => screenText(output.stdout())).toContain('content/doc-29.typ')
  running.child.stdin.write('q')

  expect((await closing).code).toBe(0)
  expect(output.stdout()).toContain('\x1b[?1049l')
})

test('interactive inspection filters rows', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await writeMinimalSite(root)
  await writeContentSources(root, 30)

  const running = new RunningProcess(
    'script',
    sizedTerminalArgs(binary, ['inspect', 'sources', '--interactive', '--color', 'never']),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  const closing = running.waitForClose(COMMAND_TIMEOUT_MS.standard)

  await expect.poll(() => screenText(output.stdout())).toContain('30 rows of 30')
  running.child.stdin.write('/Doc 29\r')
  await expect.poll(() => screenText(output.stdout())).toContain('1 row of 30')
  running.child.stdin.write('q')

  expect((await closing).code).toBe(0)
})
