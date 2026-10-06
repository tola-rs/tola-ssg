import { expect } from '@playwright/test'
import { chmod, mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { startGate } from '../../support/gate.ts'
import {
  hookJournal,
  hookToml,
  readHookJournal,
  writeHookConfiguration,
  writeUnreadContentFile,
} from '../../support/hooks.ts'
import { diagnosticRounds, readSessionRecords } from '../../support/log.ts'
import { test } from '../../support/server.ts'
import { currentRevision, type TolaWindow } from '../../support/reload.ts'

test('failed first build recovers', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: '#panic("Initial document error")\n' },
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(503)
  await page.waitForFunction(() => (window as TolaWindow).Tola?.status.errors > 0)
  expect(await currentRevision(page)).toBeNull()
  await expect(page.locator('body')).toContainText('Build failed')

  const navigation = page.waitForEvent('load')
  await site.writeContent('index.typ', 'Recovered first revision.\n')
  await navigation

  await expect(page.locator('body')).toContainText('Recovered first revision.')
  expect(await currentRevision(page)).toMatch(/^[0-9a-f]{64}$/)
  expect(await page.evaluate(() => (window as TolaWindow).Tola.awaitingPublication)).toBe(false)
})

test('unreadable source recovers after chmod', async ({ page, sites }) => {
  test.skip(
    process.platform === 'win32' || process.getuid?.() === 0,
    'Requires Unix file permissions enforced for the current user.',
  )
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'Recovered readable source.\n' },
    beforeStart: (root) => chmod(join(root, 'content/index.typ'), 0),
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(503)
  await chmod(join(site.root, 'content/index.typ'), 0o600)
  await expect(page.locator('body')).toContainText('Recovered readable source.')
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()
})

test('page reconnects after server restart', async ({ page, sites }) => {
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'Before the server restart.\n' },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('body')).toContainText('Before the server restart.')
  const originalNavigation = await page.evaluate(() => performance.timeOrigin)
  const reloaded = page.waitForEvent('load', { timeout: 15_000 })
  await site.restart()
  await reloaded
  expect(await page.evaluate(() => performance.timeOrigin)).not.toBe(originalNavigation)
  await expect(page.locator('body')).toContainText('Before the server restart.')

  await site.writeContent('index.typ', 'Connected to the restarted server.\n')
  await expect(page.locator('body')).toContainText('Connected to the restarted server.')
})

test('failed generator reruns on input', async ({ page, sites }) => {
  let invocations = ''
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: '#read("../inputs/title.txt")\n' },
    beforeStart: async (root) => {
      const input = join(root, 'inputs/title.txt')
      const trigger = join(root, 'generator.txt')
      await mkdir(join(root, 'inputs'))
      invocations = join(dirname(root), 'generator-invocations.txt')
      await writeFile(trigger, 'failed')
      const command = join(root, 'generate-title.mjs')
      await writeFile(
        command,
        `
        import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
        const trigger = readFileSync(${JSON.stringify(trigger)}, 'utf8');
        appendFileSync(${JSON.stringify(invocations)}, trigger + '\\n');
        writeFileSync(${JSON.stringify(input)}, 'Recovered after the watched generator input changed.');
        if (trigger === 'failed') {
          process.stderr.write('Initial generator stopped after writing its input');
          process.exit(7);
        }
      `,
      )
      await writeHookConfiguration(root, [hookToml('before-build', command, {
        name: 'generator',
        outputs: ['inputs'],
        rerunOn: ['generator.txt'],
      })])
    },
  })
  expect(site.stderr()).toContain('Initial generator stopped after writing its input')
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(503)
  await page.waitForFunction(() => (window as TolaWindow).Tola?.status.errors > 0)
  expect(await currentRevision(page)).toBeNull()
  expect(await readFile(invocations, 'utf8')).toBe('failed\n')

  const navigation = page.waitForEvent('load')
  await writeFile(join(site.root, 'generator.txt'), 'ready')
  await navigation

  await expect(page.locator('body')).toContainText('Recovered after the watched generator input changed.')
  await expect(page.locator('#tola-dev-status')).not.toBeVisible()
  expect(await readFile(invocations, 'utf8')).toBe('failed\nready\n')
})

test('failed generator write does not rerun the chain', async ({ page, sites }) => {
  let invocations!: string
  const site = await sites.dev({
    initialContent: { relativePath: 'index.typ', source: 'Never served.\n' },
    beforeStart: async (root) => {
      invocations = hookJournal(root)
      const command = join(root, 'fail-after-writing.mjs')
      await writeFile(
        command,
        `
        import { appendFileSync, writeFileSync } from 'node:fs';
        appendFileSync(${JSON.stringify(invocations)}, 'run\\n');
        writeFileSync('content/extra.txt', 'Written before the generator failed.\\n');
        process.stderr.write('Generator stopped before it produced a source');
        process.exit(7);
      `,
      )
      await writeUnreadContentFile(root)
      await writeHookConfiguration(root, [hookToml('before-build', command, { name: 'generator' })])
    },
  })
  const response = await page.goto(site.url, { waitUntil: 'load' })
  expect(response?.status()).toBe(503)
  await expect(page.locator('body')).toContainText('Build failed')
  await expect.poll(() => site.stderr()).toContain('Generator stopped before it produced a source')
  expect(await readHookJournal(invocations)).toBe('run\n')

  await site.writeContent('index.typ', 'Still failing after the edit.\n')
  await expect.poll(() => readHookJournal(invocations)).toBe('run\nrun\n')
  // Every failing round keeps its report in the session log; a piped transcript writes the
  // identical block once, so the log is where a repeated round is counted.
  await expect
    .poll(async () => diagnosticRounds(await readSessionRecords(site.root), 'hook.command'))
    .toEqual([1, 2])
})

test('obsolete initial candidate never serves', async ({ page, sites }) => {
  let reportInitial!: (html: string) => void
  const initialCandidate = new Promise<string>((resolve) => {
    reportInitial = resolve
  })
  let reportReplacement!: (html: string) => void
  const replacementCandidate = new Promise<string>((resolve) => {
    reportReplacement = resolve
  })
  let failGate!: (error: Error) => void
  const gateFailure = new Promise<never>((_, reject) => {
    failGate = reject
  })
  let receivedInitial = false
  let initialHookClosed = false
  const gate = await startGate((request, response) => {
    let html = ''
    request.setEncoding('utf8')
    request.on('data', (chunk) => {
      html += chunk
    })
    request.on('error', (error) => {
      if (!request.complete) failGate(error)
    })
    request.once('end', () => {
      if (!receivedInitial) {
        receivedInitial = true
        response.once('close', () => {
          initialHookClosed = true
        })
        reportInitial(html)
      } else {
        reportReplacement(html)
        response.end('continue')
      }
    })
  })
  gate.server.on('error', failGate)

  let documentPath: string | undefined
  let editedDocument: string | undefined
  try {
    const starting = sites.dev({
      initialContent: { relativePath: 'index.typ', source: 'Before the initial edit.\n' },
      beforeStart: async (root) => {
        documentPath = join(root, 'content/index.typ')
        editedDocument = join(dirname(root), 'initial-edit.typ')
        await writeFile(editedDocument, 'Edited while the first build was running.\n')
        const command = join(root, 'inspect-initial-candidate.mjs')
        await writeFile(
          command,
          `
          import { writeFileSync } from 'node:fs';
          import { readFile } from 'node:fs/promises';
          import { join } from 'node:path';
          const html = await readFile(join(process.env.TOLA_HOOK_INPUT_DIR, 'index.html'), 'utf8');
          const response = await fetch(${JSON.stringify(gate.url)}, {
            method: 'POST',
            body: html,
          });
          if (!response.ok) throw new Error('Candidate gate rejected the build');
          await response.text();
          writeFileSync(join(process.env.TOLA_HOOK_OUTPUT_DIR, 'gate.txt'), 'gate reached');
        `,
        )
        await writeHookConfiguration(root, [
          hookToml('generate-outputs', command, { name: 'gate', outputs: [{ file: 'gate.txt' }] }),
        ])
      },
    })
    const prematureServing = starting.then(() => {
      throw new Error('Dev served before the initial candidate was observed')
    })
    const first = await Promise.race([initialCandidate, gateFailure, prematureServing])
    expect(first).toContain('Before the initial edit.')
    if (!documentPath || !editedDocument) throw new Error('Initial document paths were not prepared')

    await rename(editedDocument, documentPath)
    const replacement = await Promise.race([replacementCandidate, gateFailure, prematureServing])
    expect(replacement).toContain('Edited while the first build was running.')
    await expect.poll(() => initialHookClosed).toBe(true)

    const site = await Promise.race([starting, gateFailure])
    await page.goto(site.url, { waitUntil: 'load' })
    await expect(page.locator('body')).toContainText('Edited while the first build was running.')
    await expect(page.locator('#tola-dev-status')).not.toBeVisible()
    expect(await currentRevision(page)).toMatch(/^[0-9a-f]{64}$/)
  } finally {
    await gate.close()
  }
})
