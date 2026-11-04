import { expect } from '@playwright/test'
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import type { ServerResponse } from 'node:http'
import { type AddressInfo, createServer } from 'node:net'
import { join } from 'node:path'
import { startGate } from '../../support/gate.ts'
import { hookToml, writeHookConfiguration } from '../../support/hooks.ts'
import { readSessionRecords, servingUrl } from '../../support/log.ts'
import {
  captureOutput,
  COMMAND_TIMEOUT_MS,
  commandRunner,
  expectExited,
  RunningProcess,
  startCommand,
} from '../../support/process.ts'
import { test } from '../../support/server.ts'
import { writeMinimalSite } from '../../support/site.ts'
import { screenText, shellQuote, sizedTerminalArgs } from '../../support/terminal.ts'

/** A listening socket holding `port` so a development startup has to fall back from it. */
async function holdPort(port: number): Promise<{ port: number; release: () => Promise<void> }> {
  const server = createServer()
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(port, () => resolve())
  })
  return {
    port: (server.address() as AddressInfo).port,
    release: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve())
      }),
  }
}

function openDevelopmentTerminal(
  binary: string,
  root: string,
  { checkModes = false, interfaceAddress = '127.0.0.1', port = 0 } = {},
) {
  const args = ['dev', '--interface', interfaceAddress, '--port', String(port), '--color', 'never']
  const invocation = [binary, ...args].map(shellQuote).join(' ')
  const command = checkModes
    ? `tola_modes=$(stty -g); ${invocation}; tola_exit=$?; ` +
      'if [ "$(stty -g)" = "$tola_modes" ]; then printf "\\nTOLA_TERMINAL_RESTORED\\n"; fi; ' +
      'exit "$tola_exit"'
    : `exec ${invocation}`
  const running = new RunningProcess(
    'script',
    sizedTerminalArgs('sh', ['-c', 'printf "TOLA_TTY=%s\\n" "$(tty)"; ' + command]),
    root,
    { TERM: 'xterm-256color' },
  )
  const output = captureOutput(running.child)
  return { running, ...output }
}

test('development errors stay in the view', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  const program = '#document("index.html")[Home]\n#document("404.html")[Missing]\n'
  await writeMinimalSite(root, { program })
  const terminal = openDevelopmentTerminal(binary, root)
  const frames = () => terminal.stdout().split('\x1b[?2026l').length - 1
  const failures = async () =>
    (await readSessionRecords(root)).filter((record) =>
      record.fields.kind === 'diagnostic' && record.fields.code === 'config.toml'
    ).length
  try {
    await expect.poll(() => screenText(terminal.stdout())).toContain('Watching for changes')
    let previousFrames = frames()
    await writeFile(join(root, 'tola.toml'), '[server]\nport = "server#"\n')
    await expect.poll(failures).toBeGreaterThan(0)
    await expect.poll(() => screenText(terminal.stdout())).toContain('error[config.toml]')
    await expect.poll(frames).toBeGreaterThan(previousFrames)

    const reported = await failures()
    previousFrames = frames()
    await writeFile(join(root, 'site.typ'), program + '\n')
    await expect.poll(failures).toBeGreaterThan(reported)
    await expect.poll(frames).toBeGreaterThan(previousFrames)
    expect(terminal.stdout().match(/error\[config\.toml\]/g)).toHaveLength(1)
  } finally {
    await terminal.running.terminate()
  }
})

test('startup warnings share one round', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await writeMinimalSite(root, {
    program: '#document("index.html")[Home]\n#document("404.html")[Missing]\n',
  })
  const web = await holdPort(0)
  const terminal = openDevelopmentTerminal(binary, root, {
    interfaceAddress: '0.0.0.0',
    port: web.port,
  })
  try {
    await expect.poll(() => screenText(terminal.stdout())).toContain('Watching for changes')
    terminal.running.child.stdin.write('\x1b[D')
    await expect.poll(() => {
      const view = screenText(terminal.stdout())
      return ['warning[server.port_in_use]', 'warning[server.network_exposed]'].every((warning) =>
        view.includes(warning)
      )
    }).toBe(true)
  } finally {
    await terminal.running.terminate()
    await web.release()
  }
})

test('development interrupt restores terminal modes', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  await writeMinimalSite(root)
  const terminal = openDevelopmentTerminal(binary, root, { checkModes: true })
  try {
    await expect.poll(() => servingUrl(root)).toBeDefined()
    terminal.running.child.stdin.write('\x03')
    expect((await terminal.running.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(130)
    expect(terminal.stdout()).toContain('TOLA_TERMINAL_RESTORED')
  } finally {
    await terminal.running.terminate()
  }
})

test('quiet preview retains warnings', async ({ binary, directory: root }) => {
  await writeMinimalSite(root, { program: '#document("index.html")[Quiet preview]\n' })
  const server = startCommand(binary, [
    'preview',
    '--quiet',
    '--interface',
    '127.0.0.1',
    '--port',
    '0',
    '--log-file',
    '.tola/state/logs/session.jsonl',
    '--color',
    'never',
  ], root)
  try {
    let url: string | undefined
    await expect.poll(async () => (url = await servingUrl(root))).toBeDefined()
    const response = await fetch(url!)
    expect(response.status).toBe(200)
    expect(await response.text()).toContain('Quiet preview')
    expect(server.stderr()).toContain('warning[site.not_found_missing]')
    const startup = (await readSessionRecords(root)).find((record) => record.fields.kind === 'summary')
    const summary = (startup?.fields as { message?: string } | undefined)?.message
    expect(summary).toEqual(expect.any(String))
    expect(server.stderr()).not.toContain(summary!)
    expect(server.stderr()).not.toContain(url!)
  } finally {
    await server.command.terminate()
  }
})

test('verbose build reports debug steps', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root, { program: '#document("index.html")[Hello from Tola.]' })

  const verbose = await run(['build', '-v'], root, COMMAND_TIMEOUT_MS.build)
  expectExited(verbose)
  expect(verbose.stdout).toBe('')
  expect(verbose.stderr).toMatch(/debug compile:/)
  expect(verbose.stderr).not.toContain('\u001b')
  expect(await readFile(join(root, 'public/index.html'), 'utf8')).toContain('Hello from Tola.')
})

test('quiet build hides progress', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root, { program: '#document("index.html")[Hello from Tola.]' })

  const quiet = await run(['build', '--quiet'], root, COMMAND_TIMEOUT_MS.build)
  expectExited(quiet)
  expect(quiet.stdout).toBe('')
  expect(quiet.stderr).not.toContain('Building site')
  expect(quiet.stderr).not.toContain('Built ')
  expect(await readFile(join(root, 'public/index.html'), 'utf8')).toContain('Hello from Tola.')
})

test('build lock contention preserves output', async ({ binary, directory: root }) => {
  test.skip(process.platform === 'win32', 'Node pipes do not deliver Windows console interrupts.')
  const run = commandRunner(binary)
  const commands: RunningProcess[] = []
  const requests = new Map<string, ServerResponse>()
  const gate = await startGate((request, response) => {
    requests.set(request.url!, response)
  })
  let teardownError: unknown
  try {
    await writeMinimalSite(root, { program: '#document("index.html")[Published before contention.]' })
    expectExited(await run(['build'], root, COMMAND_TIMEOUT_MS.build))
    const published = await readFile(join(root, 'public/index.html'), 'utf8')
    const generator = join(root, 'generator.mjs')
    await writeFile(
      generator,
      `
      const response = await fetch(${JSON.stringify(`${gate.url}/`)} + process.env.BUILD_LOCK_CASE);
      await response.text();
    `,
    )
    await writeHookConfiguration(root, [hookToml('before-build', generator, { name: 'generator' })])
    const startBuild = (name: string) => {
      const started = startCommand(binary, ['build', '--color', 'never'], root, { BUILD_LOCK_CASE: name })
      commands.push(started.command)
      return started
    }
    const owner = startBuild('owner')
    await expect.poll(() => requests.has('/owner')).toBe(true)
    await rm(join(root, '.tola'), { recursive: true, force: true })
    const cancelled = startBuild('cancelled')
    await expect.poll(cancelled.stderr).toMatch(/waiting.+build/i)
    expect([...requests.keys()]).toEqual(['/owner'])
    cancelled.command.interrupt()
    expect((await cancelled.command.waitForClose(5_000)).code).toBe(130)
    expect(await readFile(join(root, 'public/index.html'), 'utf8')).toBe(published)
    const successor = startBuild('successor')
    await expect.poll(successor.stderr).toMatch(/waiting.+build/i)
    expect([...requests.keys()]).toEqual(['/owner'])
    requests.get('/owner')!.end('continue')
    expect((await owner.command.waitForClose(30_000)).code).toBe(0)
    await expect.poll(() => requests.has('/successor')).toBe(true)
    requests.get('/successor')!.end('continue')
    expect((await successor.command.waitForClose(30_000)).code).toBe(0)
    expect(await readFile(join(root, 'public/index.html'), 'utf8')).toBe(published)
    expect(requests.has('/cancelled')).toBe(false)
  } finally {
    for (const response of requests.values()) response.end('continue')
    const terminated = await Promise.allSettled(commands.map((command) => command.terminate()))
    await gate.close()
    const errors = terminated.flatMap((result) => result.status === 'rejected' ? [result.reason] : [])
    if (errors.length) teardownError = new AggregateError(errors, 'Build command teardown failed')
  }
  if (teardownError !== undefined) throw teardownError
})

test('an interrupted publication is completed before compilation', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root, { program: '#document("index.html")[Last successful publication.]' })
  expectExited(await run(['build'], root, COMMAND_TIMEOUT_MS.build))

  const workspace = join(root, '.public-swap')
  const recovered = 'The verified candidate.'
  await mkdir(join(workspace, 'staged/0'), { recursive: true })
  await mkdir(join(workspace, 'previous'), { recursive: true })
  await writeFile(join(workspace, 'staged/0/index.html'), recovered)
  await rename(join(root, 'public'), join(workspace, 'previous/0'))
  await writeFile(join(workspace, 'owner'), 'tola-swap\ntarget=public\n')
  await writeFile(join(workspace, 'commit'), 'public\n')
  await writeFile(join(root, 'site.typ'), '#panic("Compilation must fail")')

  expect((await run(['build'], root, COMMAND_TIMEOUT_MS.build)).code).not.toBe(0)
  expect(await readFile(join(root, 'public/index.html'), 'utf8')).toBe(recovered)
  await expect(readFile(join(workspace, 'owner'))).rejects.toMatchObject({ code: 'ENOENT' })
})

test('consumer can check the same site', async ({ binary, directory: root }) => {
  let checkGate: ServerResponse | undefined
  let buildProcess: RunningProcess | undefined
  const gate = await startGate((_request, response) => {
    checkGate = response
  })
  try {
    await writeMinimalSite(root, { program: '#document("index.html")[Published before the consumer.]' })
    const consumer = join(root, 'check-published.mjs')
    await writeFile(
      consumer,
      `
      import { spawnSync } from 'node:child_process';
      const checkExit = spawnSync(${JSON.stringify(binary)}, ['check', '--color', 'never'], {
        cwd: process.cwd(),
        stdio: 'inherit',
});
      if (checkExit.error) throw checkExit.error;
      if (checkExit.status !== 0) process.exit(checkExit.status ?? 1);
      const response = await fetch(${JSON.stringify(gate.url)});
      await response.text();
    `,
    )
    await writeHookConfiguration(root, [hookToml('after-publish', consumer, { name: 'check-published' })])
    const build = startCommand(binary, ['build', '--color', 'never'], root)
    buildProcess = build.command
    await expect.poll(() => checkGate !== undefined, { timeout: COMMAND_TIMEOUT_MS.build }).toBe(true)
    expect(await readFile(join(root, 'public/index.html'), 'utf8')).toContain(
      'Published before the consumer.',
    )
    checkGate!.end('continue')
    expectExited({ ...await buildProcess.waitForClose(COMMAND_TIMEOUT_MS.build), stderr: build.stderr() })
  } finally {
    checkGate?.end()
    try {
      if (buildProcess && buildProcess.exit === undefined) {
        buildProcess.interrupt()
        await buildProcess.waitForClose(5_000)
      }
    } finally {
      await gate.close()
      await buildProcess?.terminate()
    }
  }
})

test('only a successful build publishes', async ({ binary, page, sites }) => {
  const run = commandRunner(binary)
  let published = ''
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'Previously published page.\n' },
    beforeStart: async (root) => {
      await writeFile(join(root, 'content/removed.typ'), 'Previously published route.\n')
      expectExited(await run(['build'], root, COMMAND_TIMEOUT_MS.build))
      published = await readFile(join(root, 'public/index.html'), 'utf8')
      await rm(join(root, 'content/removed.typ'))
      await writeFile(join(root, 'content/index.typ'), 'Current development page.\n')
    },
  })
  await page.goto(site.url)
  await expect(page.locator('body')).toContainText('Current development page.')
  const reloadScript = await page.locator('script[src$="/hotreload.js"]').getAttribute('src')
  expect(reloadScript).not.toBeNull()
  expect(await readFile(join(site.root, 'public/index.html'), 'utf8')).toBe(published)
  expect(await readFile(join(site.root, 'public/removed/index.html'), 'utf8')).toContain(
    'Previously published route.',
  )
  const written = await run(['build'], site.root, COMMAND_TIMEOUT_MS.build)
  expectExited(written)
  await expect(readFile(join(site.root, 'public/removed/index.html'))).rejects.toMatchObject({
    code: 'ENOENT',
  })
  const production = await readFile(join(site.root, 'public/index.html'), 'utf8')
  expect(production).toContain('Current development page.')
  expect(production).not.toContain(reloadScript!)
  await expect(page.locator('body')).toContainText('Current development page.')
  await site.writeContent('index.typ', '#panic("Reject this candidate")\n')
  expect((await run(['build'], site.root, COMMAND_TIMEOUT_MS.build)).code).not.toBe(0)
  expect(await readFile(join(site.root, 'public/index.html'), 'utf8')).toBe(production)
  await expect(page.locator('body')).toContainText('Current development page.')
})

test('neither check nor preview publishes', async ({ binary, directory, page, request, sites }) => {
  const run = commandRunner(binary)
  const journal = join(directory, 'production-hooks.txt')
  const afterPublish = join(directory, 'after-publish.txt')
  const site = await sites.preview({
    initialContent: { relativePath: 'index.typ', source: 'Previously published page.\n' },
    beforeStart: async (root) => {
      expectExited(await run(['build'], root, COMMAND_TIMEOUT_MS.build))
      const published = await readFile(join(root, 'public/index.html'), 'utf8')
      await writeFile(join(root, 'public/keep.txt'), 'Existing public files survive check.')
      const before = join(root, 'before.mjs')
      const generate = join(root, 'generate.mjs')
      await writeFile(
        before,
        `
        import { appendFileSync, writeFileSync } from 'node:fs';
        writeFileSync('content/index.typ', 'Fresh production source.\\n');
        appendFileSync(${JSON.stringify(journal)}, 'before\\n');
      `,
      )
      await writeFile(
        generate,
        `
        import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
        import { join } from 'node:path';
        const html = readFileSync(join(process.env.TOLA_HOOK_INPUT_DIR, 'index.html'), 'utf8');
        if (!html.includes('Fresh production source.')) throw new Error('Missing before-build source');
        if (html.includes('/hotreload.js')) throw new Error('Expected a production candidate');
        writeFileSync(join(process.env.TOLA_HOOK_OUTPUT_DIR, 'search.txt'), 'Fresh production search.');
        appendFileSync(${JSON.stringify(journal)}, 'generate\\n');
      `,
      )
      const after = join(root, 'after-publish.mjs')
      await writeFile(
        after,
        `
        import { appendFileSync } from 'node:fs';
        appendFileSync(${JSON.stringify(afterPublish)}, 'after\\n');
      `,
      )
      await writeHookConfiguration(root, [
        hookToml('before-build', before, { name: 'before', dev: 'skip', outputs: ['content/index.typ'] }),
        hookToml('generate-outputs', generate, {
          name: 'generate',
          dev: 'skip',
          outputs: [{ file: 'search.txt' }],
        }),
        hookToml('after-publish', after, { name: 'after', dev: 'run' }),
      ])
      const checked = await run(['check'], root, COMMAND_TIMEOUT_MS.build)
      expectExited(checked)
      expect(await readFile(journal, 'utf8')).toBe('before\ngenerate\n')
      expect(await readFile(join(root, 'public/index.html'), 'utf8')).toBe(published)
      expect(await readFile(join(root, 'public/keep.txt'), 'utf8')).toBe(
        'Existing public files survive check.',
      )
      await expect(readFile(join(root, 'public/search.txt'))).rejects.toMatchObject({ code: 'ENOENT' })
      await expect(readFile(afterPublish)).rejects.toMatchObject({ code: 'ENOENT' })
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Fresh production source.')
  await expect(page.locator('script[src$="/hotreload.js"]')).toHaveCount(0)
  const generated = await request.get(new URL('/search.txt', site.url).href)
  expect(generated.status()).toBe(200)
  expect(await generated.text()).toBe('Fresh production search.')
  expect(await readFile(journal, 'utf8')).toBe('before\ngenerate\nbefore\ngenerate\n')
  await site.close()
  await expect(readFile(afterPublish)).rejects.toMatchObject({ code: 'ENOENT' })
})

test('skill files match selected stdout', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  await writeFile(join(directory, 'tola.toml'), 'not valid TOML')
  const selections = [[], ['math'], ['math', 'numbering'], ['--list'], ['--all']]
  for (const [index, selection] of selections.entries()) {
    const printed = await run(['skill', ...selection], directory, COMMAND_TIMEOUT_MS.short)
    expectExited(printed)
    const file = index === 0 ? 'guide.md' : join('guides', 'guide-' + index + '.md')
    const exported = await run(['skill', ...selection, '-o', file], directory, COMMAND_TIMEOUT_MS.short)
    expectExited(exported)
    expect(exported.stdout).toBe('')
    expect(await readFile(join(directory, file), 'utf8')).toBe(printed.stdout)
  }
})

test('skill output preserves existing files', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  const file = join(directory, 'guide.md')
  await writeFile(file, 'Site-specific authoring guidance.\n')
  const conflict = await run(['skill', '--all', '-o', file], directory, COMMAND_TIMEOUT_MS.short)
  expect(conflict.code).not.toBe(0)
  expect(await readFile(file, 'utf8')).toBe('Site-specific authoring guidance.\n')
})

test('failed skill selection writes nothing', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  const file = join(directory, 'missing.md')
  const missing = await run(['skill', 'math', 'missing', '-o', file], directory, COMMAND_TIMEOUT_MS.short)
  expect(missing.code).not.toBe(0)
  expect(missing.stdout).toBe('')
  expect(missing.stderr).toContain('tola skill --list')
  await expect(readFile(file)).rejects.toMatchObject({ code: 'ENOENT' })
})

test('log collision stops skill output', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  const file = join(directory, 'guide.md')
  const conflict = await run(
    ['skill', '--output', file, '--log-file', file],
    directory,
    COMMAND_TIMEOUT_MS.short,
  )
  expect(conflict.code).not.toBe(0)
  await expect(readFile(file)).rejects.toMatchObject({ code: 'ENOENT' })
})

test('version flag prints both releases', async ({ binary, directory }) => {
  const run = commandRunner(binary)
  const version = await run(['--version'], directory, COMMAND_TIMEOUT_MS.short)
  expectExited(version)
  expect(version.stdout).toMatch(/^tola \d+\.\d+\.\d+\ntypst \d+\.\d+\.\d+\n$/)
})
