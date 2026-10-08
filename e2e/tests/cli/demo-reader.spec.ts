import { expect, type TestInfo } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { createConnection } from 'node:net'
import { join } from 'node:path'
import { captureOutput, COMMAND_TIMEOUT_MS, RunningProcess, test } from '../../support/process.ts'
import { screenText, sizedTerminalArgs } from '../../support/terminal.ts'

function demoReader(binary: string, root: string, verbose = false) {
  const args = [
    'help',
    '-i',
    'demo',
    'backlinks',
    '--lang',
    'en',
    '--color',
    'never',
    '--no-log-file',
    ...(verbose ? ['-vv'] : []),
  ]
  const started = performance.now()
  const running = new RunningProcess('script', sizedTerminalArgs(binary, args), root, {
    TERM: 'xterm-256color',
  })
  const output = captureOutput(running.child)
  return { running, output, started, screen: () => screenText(output.stdout()) }
}

type Reader = ReturnType<typeof demoReader>

async function firstFrame(reader: Reader): Promise<void> {
  await expect.poll(() => reader.screen(), { timeout: COMMAND_TIMEOUT_MS.standard }).toContain('[Preview]')
}

async function attachFailure(reader: Reader, testInfo: TestInfo): Promise<void> {
  await Promise.all([
    testInfo.attach('demo-reader-stdout', { body: reader.output.stdout(), contentType: 'text/plain' }),
    testInfo.attach('demo-reader-stderr', { body: reader.output.stderr(), contentType: 'text/plain' }),
    testInfo.attach('demo-reader-process', {
      body: JSON.stringify(
        {
          elapsedMs: performance.now() - reader.started,
          command: reader.running.child.spawnargs,
          pid: reader.running.child.pid ?? null,
          exit: reader.running.exit ?? null,
          error: reader.running.error?.message ?? null,
        },
        null,
        2,
      ),
      contentType: 'application/json',
    }),
  ])
}

function previewUrl(screen: string): string | undefined {
  return screen.match(/\bReady\s*·\s*(http:\/\/127\.0\.0\.1:\d+\/[^\s]*)/)?.[1]
}

async function ready(reader: Reader): Promise<string> {
  await expect.poll(() => previewUrl(reader.screen()), { timeout: COMMAND_TIMEOUT_MS.build }).toBeDefined()
  const url = previewUrl(reader.screen())!
  const response = await fetch(url, { signal: AbortSignal.timeout(COMMAND_TIMEOUT_MS.short) })
  expect(response.status).toBe(200)
  expect(await response.text()).toContain('Body links become backlinks')
  return url
}

async function portIsOpen(address: string): Promise<boolean> {
  const url = new URL(address)
  return await new Promise<boolean>((resolve, reject) => {
    const socket = createConnection({ host: url.hostname, port: Number(url.port) })
    const finish = (opened: boolean) => {
      socket.destroy()
      resolve(opened)
    }
    socket.setTimeout(COMMAND_TIMEOUT_MS.short)
    socket.once('connect', () => finish(true))
    socket.once('error', (error) => {
      if ((error as NodeJS.ErrnoException).code === 'ECONNREFUSED') finish(false)
      else {
        socket.destroy()
        reject(error)
      }
    })
    socket.once('timeout', () => {
      socket.destroy()
      reject(new Error(`Preview port did not answer: ${address}`))
    })
  })
}

function clickText(reader: Reader, text: string): void {
  const rows = reader.screen().split('\n')
  const row = rows.findIndex((line) => line.includes(text))
  expect(row, `No visible ${text} button:\n${reader.screen()}`).toBeGreaterThanOrEqual(0)
  const column = rows[row]!.indexOf(text) + Math.floor(text.length / 2)
  reader.running.child.stdin.write(`\x1b[<0;${column + 1};${row + 1}M\x1b[<0;${column + 1};${row + 1}m`)
}

async function openBacklinkSource(reader: Reader): Promise<string> {
  const path = 'site/backlinks.typ'
  reader.running.child.stdin.write(`/${path}`)
  await expect.poll(() => reader.screen().split('\n').slice(0, -3).join('\n')).toContain(path)
  reader.running.child.stdin.write('\r')
  await expect.poll(() => reader.screen()).toContain('[Preview]')
  await expect.poll(() => reader.screen().split('\n').slice(1, -3).join('\n')).toContain(path)
  const rows = reader.screen().split('\n')
  const row = rows.findIndex((line, index) => index < rows.length - 3 && line.includes(path))
  expect(row, reader.screen()).toBeGreaterThanOrEqual(0)
  const original = rows[row]!
  const position = rows.slice(1, 9).join('\n')

  reader.running.child.stdin.write('\t')
  await expect.poll(() => reader.screen()).toContain('press a label')
  const tail = '/backlinks.typ'
  const labeled = reader.screen().split('\n').find((line) => line.includes(tail))
  expect(labeled, reader.screen()).toBeDefined()
  const start = labeled!.indexOf(tail) - 'site'.length
  let keys = ''
  for (let column = Math.max(0, start - 2); column < start + 3; column += 1) {
    const key = labeled![column]
    if (key !== undefined && key !== original[column] && 'asdfghjkl'.includes(key)) keys += key
  }
  expect(keys, `No source-link label:\n${reader.screen()}`).toMatch(/^[asdfghjkl]{1,2}$/)
  reader.running.child.stdin.write(keys)
  await expect.poll(() => reader.screen()).toContain('Incoming links from page bodies · site/backlinks.typ')
  return position
}

test('demo preview stays live across reader navigation', async ({ binary, directory: root }, testInfo) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  test.setTimeout(COMMAND_TIMEOUT_MS.build * 2)
  const reader = demoReader(binary, root, true)
  try {
    await firstFrame(reader)
    reader.running.child.stdin.write('p')
    const first = await ready(reader)
    reader.running.child.stdin.write('j')
    await expect.poll(() => reader.screen()).not.toContain('Previewing')
    reader.running.child.stdin.write('p')
    await expect.poll(() => reader.screen()).toContain('Previewing')
    expect(await ready(reader)).toBe(first)

    const position = await openBacklinkSource(reader)
    reader.running.child.stdin.write('/from-within')
    await expect.poll(() => reader.screen().split('\n').slice(-3).join('\n')).toContain('from-within')
    reader.running.child.stdin.write('\r')
    await expect.poll(() => reader.screen()).toContain('[Preview]')
    expect(await ready(reader)).toBe(first)
    reader.running.child.stdin.write('\x1b')
    await expect.poll(() => reader.screen().split('\n').slice(1, 9).join('\n')).toBe(position)
    expect(await ready(reader)).toBe(first)

    reader.running.child.stdin.write('x')
    await expect.poll(() => reader.screen()).toContain('Stopped')
    await expect.poll(() => portIsOpen(first)).toBe(false)
    reader.running.child.stdin.write('p')
    const restarted = await ready(reader)
    reader.running.child.stdin.write('q')
    expect((await reader.running.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(0)
    await expect.poll(() => portIsOpen(restarted)).toBe(false)
  } catch (error) {
    await attachFailure(reader, testInfo)
    throw error
  } finally {
    await reader.running.terminate()
  }
})

test('mouse preview cancels with the reader', async ({ binary, directory: root }, testInfo) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  test.setTimeout(COMMAND_TIMEOUT_MS.build * 2)
  const reader = demoReader(binary, root)
  try {
    await firstFrame(reader)
    clickText(reader, '[Preview]')
    const url = await ready(reader)
    reader.running.child.stdin.write('\x03')
    expect((await reader.running.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(130)
    await expect.poll(() => portIsOpen(url)).toBe(false)
  } catch (error) {
    await attachFailure(reader, testInfo)
    throw error
  } finally {
    await reader.running.terminate()
  }
})

test('reader export preserves an edited destination', async ({ binary, directory: root }, testInfo) => {
  test.skip(process.platform === 'win32', 'requires a POSIX pseudo-terminal')
  const reader = demoReader(binary, root)
  const source = join(root, 'copy', 'site.typ')
  try {
    await firstFrame(reader)
    expect(reader.screen()).toContain('[Export]')
    reader.running.child.stdin.write('e')
    await expect.poll(() => reader.screen()).toContain('export directory')
    reader.running.child.stdin.write('copy\r')
    await expect.poll(() => reader.screen()).toContain('Exported to')
    const exported = await readFile(source, 'utf8')
    expect(exported).toContain('incoming-pages')
    const edited = `${exported}\n#let exported-change = true\n`
    await writeFile(source, edited)

    reader.running.child.stdin.write('e')
    await expect.poll(() => reader.screen()).toContain('export directory')
    reader.running.child.stdin.write('copy\r')
    await expect.poll(() => reader.screen()).toContain('already exists')
    expect(await readFile(source, 'utf8')).toBe(edited)
    expect(reader.running.exit).toBeUndefined()
    reader.running.child.stdin.write('q')
    expect((await reader.running.waitForClose(COMMAND_TIMEOUT_MS.standard)).code).toBe(0)
  } catch (error) {
    await attachFailure(reader, testInfo)
    throw error
  } finally {
    await reader.running.terminate()
  }
})
