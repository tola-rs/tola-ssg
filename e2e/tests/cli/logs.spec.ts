import { expect } from '@playwright/test'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { hookToml, writeHookConfiguration } from '../../support/hooks.ts'
import { type LogRecord, readLogRecords, readSessionRecords } from '../../support/log.ts'
import { COMMAND_TIMEOUT_MS, expectExited, runCommand } from '../../support/process.ts'
import { test } from '../../support/server.ts'
import { writeMinimalSite } from '../../support/site.ts'

const SESSION_LOG_TIMEOUT_MS = 20_000

test('compact call traces retain full log ranges', async ({ binary, directory: root }) => {
  await writeMinimalSite(root, {
    program: `#let fail(body) = panic("the site publishes no landing page")
#fail[
  first body line
  middle body line
  last body line
]
`,
  })
  const log = join(root, '.tola/logs/diagnostics.jsonl')
  const result = await runCommand(
    binary,
    ['check', '--log-file', log, '--color', 'never'],
    root,
    COMMAND_TIMEOUT_MS.build,
  )
  expectExited(result, 1)
  expect(result.stderr).toContain('the site publishes no landing page')
  expect(result.stderr).toContain('site.typ:2:')
  expect(result.stderr).not.toContain('middle body line')
  const records = (await readLogRecords(log)).filter((record) => record.fields.kind === 'diagnostic')
  expect(records).toHaveLength(1)
  expect(records[0]!.fields).toMatchObject({
    diagnostic: {
      message: 'panicked with: the site publishes no landing page',
      trace: [{
        location: {
          path: 'site.typ',
          range: { start: { line: 1 }, end: { line: 5 } },
          source_lines: expect.arrayContaining([
            expect.objectContaining({ line: 4, text: '  middle body line' }),
          ]),
        },
      }],
    },
  })
})

test('shared package warning names each importing source once', async ({ binary, directory: root }) => {
  await writeMinimalSite(root, {
    program: `#document("index.html")[
  #include "content/a.typ"
  #include "content/b.typ"
]
#document("404.html")[Missing page]
`,
  })
  const packages = join(root, 'packages')
  const packageRoot = join(packages, 'local/warning/0.1.0')
  await mkdir(packageRoot, { recursive: true })
  await writeFile(
    join(packageRoot, 'typst.toml'),
    '[package]\nname = "warning"\nversion = "0.1.0"\nentrypoint = "lib.typ"\n',
  )
  await writeFile(join(packageRoot, 'lib.typ'), '#set text(font: "MissingFontXYZ")\n')
  for (const name of ['a', 'b']) {
    await writeFile(join(root, `content/${name}.typ`), '#import "@local/warning:0.1.0"\n')
  }
  const log = join(root, '.tola/logs/diagnostics.jsonl')
  const result = await runCommand(
    binary,
    ['check', '--package-path', packages, '--log-file', log, '--color', 'never'],
    root,
    COMMAND_TIMEOUT_MS.build,
  )
  expectExited(result)
  const warnings = (await readLogRecords(log)).filter((record) =>
    record.fields.kind === 'diagnostic' && record.fields.code === 'typst.compile'
  )
  expect(warnings).toHaveLength(1)
  expect(warnings[0]!.fields).toMatchObject({
    diagnostic: {
      imported_by: ['content/a.typ', 'content/b.typ'],
    },
  })
  expect(result.stderr.match(/warning\[typst\.compile\]/g)).toHaveLength(1)
})

test('export failures retain compiler diagnostics', async ({ binary, directory: root }) => {
  await writeMinimalSite(root, {
    program: `#document("index.html")[#text(font:"missing-font-qxyz")[Hi]]
#document("file.pdf",format:"pdf")[#pdf.attach("bad.txt",bytes("hi"),mime-type:"invalid") Hi]
`,
  })
  const log = join(root, '.tola/logs/export.jsonl')
  const result = await runCommand(
    binary,
    ['build', '--log-file', log, '--color', 'never'],
    root,
    COMMAND_TIMEOUT_MS.build,
  )
  expectExited(result, 1)
  const diagnostics = (await readLogRecords(log))
    .filter((record) => record.fields.kind === 'diagnostic')
    .map((record) =>
      (record.fields as typeof record.fields & {
        diagnostic: {
          code: string
          severity: string
          message: string
          location: { path: string; line: number; column: number } | null
        }
      }).diagnostic
    )
  expect(diagnostics).toEqual(expect.arrayContaining([
    expect.objectContaining({
      code: 'typst.bundle_export',
      severity: 'error',
      location: expect.objectContaining({ path: 'site.typ', line: 2, column: 37 }),
    }),
    expect.objectContaining({
      severity: 'warning',
      message: expect.stringContaining('missing-font-qxyz'),
      location: expect.objectContaining({ path: 'site.typ', line: 1 }),
    }),
  ]))
  expect(result.stderr).toContain('site.typ:2:36')
})

test('doctor json stays out of the log', async ({ binary, directory: root }) => {
  const result = await runCommand(
    binary,
    [
      'doctor',
      '--json',
      '--config',
      'missing.toml',
      '--log-file',
      '.tola/logs/session.jsonl',
      '--color',
      'always',
    ],
    root,
    COMMAND_TIMEOUT_MS.short,
  )
  expectExited(result, 1)
  const { stdout, stderr } = result
  const report = JSON.parse(stdout)
  expect(report.environment).toEqual({
    tola_version: expect.any(String),
    typst_version: expect.any(String),
    os: expect.any(String),
    arch: expect.any(String),
  })
  const configurationFailure = report.diagnostics.find((diagnostic: { severity: string; code: string }) =>
    diagnostic.severity === 'error' && diagnostic.code.startsWith('config.')
  )
  expect(configurationFailure).toBeDefined()
  expect(stderr).toContain(configurationFailure.code)
  expect(stderr).toContain('\u001b[')
  expect(stdout).not.toContain('\u001b')

  const log = join(root, '.tola/logs/session.jsonl')
  const source = await readFile(log, 'utf8')
  const records = await readLogRecords(log)
  expect(records[0]!.fields).toMatchObject({ kind: 'command_started', command: 'doctor' })
  expect(records.at(-1)!.fields).toMatchObject({ kind: 'command_finished', success: false })
  expect(records.filter((record) =>
    record.fields?.kind === 'diagnostic' &&
    record.fields.code === configurationFailure.code
  )).toHaveLength(1)
  expect(source).not.toContain('\u001b')
  expect(source).not.toContain('\\u001b')
  expect(source).not.toContain('tola_version')
})

test('interleaved hook events name their round and revision', async ({ sites, directory }) => {
  const release = join(directory, 'consumer-release.txt')
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'First page.\n' },
    beforeStart: async (root) => {
      const echo = join(root, 'echo-stages.mjs')
      await writeFile(
        echo,
        `
        process.stdout.write('echo stdout\\n');
        process.stderr.write('echo stderr\\n');
      `,
      )
      // The consumer waits for a file outside the site, so the test decides when it ends.
      const consumer = join(root, 'consume-published.mjs')
      await writeFile(
        consumer,
        `
        import { existsSync } from 'node:fs';
        process.stdout.write('consumer stdout start\\n');
        process.stderr.write('consumer stderr start\\n');
        while (!existsSync(${JSON.stringify(release)})) await new Promise(resolve => setTimeout(resolve, 20));
        process.stdout.write('consumer stdout end\\n');
        process.stderr.write('consumer stderr end\\n');
      `,
      )
      await writeHookConfiguration(root, [
        hookToml('before-build', echo, { name: 'echo', dev: 'run' }),
        hookToml('after-publish', consumer, { name: 'consumer', dev: 'run' }),
      ])
    },
  })

  const wrote = (records: LogRecord[], hook: string, fragment: string) =>
    records.some(
      (record) => record.fields.hook === hook && record.fields.output?.includes(fragment),
    )

  // Round one publishes and its consumer starts; the log holds both while the consumer waits.
  await expect.poll(
    async () => wrote(await readSessionRecords(site.root), 'consumer', 'consumer stdout start'),
    { timeout: SESSION_LOG_TIMEOUT_MS },
  ).toBe(true)

  await site.writeContent('index.typ', 'Second page.\n')
  // A second round publishes while that consumer is still running.
  await expect.poll(
    async () =>
      (await readSessionRecords(site.root)).some((record) =>
        record.fields.kind === 'summary' && record.fields.round === 2
      ),
    { timeout: SESSION_LOG_TIMEOUT_MS },
  ).toBe(true)
  await writeFile(release, '')
  await expect.poll(
    async () => wrote(await readSessionRecords(site.root), 'consumer', 'consumer stdout end'),
    { timeout: SESSION_LOG_TIMEOUT_MS },
  ).toBe(true)

  const records = await readSessionRecords(site.root)
  const hooks = records.filter((record) =>
    record.target === 'tola::hook_status' || record.target === 'tola::hook_output'
  )
  expect(hooks).not.toHaveLength(0)
  // Every hook event names the round that ran it, so a consumer outliving its round stays placeable.
  expect(hooks.filter((record) => typeof record.span?.round !== 'number')).toEqual([])

  const roundsOf = (hook: string) =>
    hooks
      .filter((record) => record.fields.hook === hook)
      .map((record) => record.span!.round)
  expect([...new Set(roundsOf('echo'))].sort()).toEqual([1, 2])

  // Every consumer event names the revision its own round published, including the run that
  // outlived round one and the run the queue kept for round two.
  const publishedRevisions = new Map(
    records
      .filter((record) => record.fields.kind === 'summary')
      .map((record) => [record.fields.round, record.fields.revision]),
  )
  const consumer = hooks.filter((record) => record.fields.hook === 'consumer')
  expect(consumer.every((record) => record.span?.revision === publishedRevisions.get(record.span!.round)))
    .toBe(true)

  // The consumer was still running when round two published: its start precedes that round's
  // summary and its end follows it.
  const secondPublished = records.findIndex((record) =>
    record.fields.kind === 'summary' && record.fields.round === 2
  )
  const wroteAt = (fragment: string) =>
    records.findIndex((record) =>
      record.fields.hook === 'consumer' && record.fields.output?.includes(fragment)
    )
  expect(wroteAt('consumer stdout start')).toBeLessThan(secondPublished)
  expect(wroteAt('consumer stdout end')).toBeGreaterThan(secondPublished)
})

test('hook output keeps its stream', async ({ sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'Page.\n' },
    beforeStart: async (root) => {
      const echo = join(root, 'echo-streams.mjs')
      await writeFile(
        echo,
        `
        process.stdout.write('echo stdout\\n');
        process.stderr.write('echo stderr\\n');
      `,
      )
      await writeHookConfiguration(root, [hookToml('before-build', echo, { name: 'echo', dev: 'run' })])
    },
  })

  const output = (records: LogRecord[], stream: string) =>
    records
      .filter((record) => record.fields.hook === 'echo' && record.fields.stream === stream)
      .map((record) => record.fields.output)
      .join('')
  await expect.poll(async () => output(await readSessionRecords(site.root), 'stdout'), {
    timeout: SESSION_LOG_TIMEOUT_MS,
  }).toContain('echo stdout')
  const records = await readSessionRecords(site.root)
  expect(output(records, 'stderr')).toContain('echo stderr')
})
