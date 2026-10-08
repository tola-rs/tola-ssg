import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import type { EventEmitter } from 'node:events'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
import type { TestInfo } from '@playwright/test'
import {
  captureOutput,
  runCommand,
  type RunningCommand,
  RunningProcess,
  startCommand,
  test as base,
} from './process.ts'

const SERVING_LINE = /^Serving (https?:\/\/[^\s]+)\r?\n/m

type RunningSite = {
  root: string
  url: string
  processId: number
  stdout: () => string
  stderr: () => string
  writeContent: (relativePath: string, source: string) => Promise<void>
  removeContent: (relativePath: string) => Promise<void>
  restart: () => Promise<void>
  close: () => Promise<void>
}

type SiteOptions = {
  contentRoot?: string
  initialContent?: { relativePath: string; source: string } | null
  packageDataRoot?: string
  packageCacheRoot?: string
  inputScope?: '--offline' | '--pure'
  /** `dev` only; `false` serves without installing revisions from filesystem changes. */
  watch?: boolean
  environment?: NodeJS.ProcessEnv
  beforeStart?: (siteRoot: string) => Promise<void>
}

function waitForServing(server: RunningCommand): Promise<string> {
  const child = server.command.child
  const channels: EventEmitter[] = [child, child.stdin, child.stdout, child.stderr]
  return new Promise((resolveUrl, reject) => {
    const cleanup = () => {
      clearTimeout(timeout)
      child.stderr.removeListener('data', observe)
      child.removeListener('close', closed)
      for (const channel of channels) {
        channel.removeListener('error', failed)
      }
    }
    const failed = (error: Error) => {
      cleanup()
      reject(error)
    }
    const closed = (code: number | null, signal: NodeJS.Signals | null) => {
      failed(
        new Error(
          `Tola server exited with ${signal ?? `code ${code}`}:\n${server.stderr()}${server.stdout()}`,
        ),
      )
    }
    const observe = () => {
      if (server.command.error) {
        failed(server.command.error)
        return
      }
      if (server.command.exit) {
        closed(server.command.exit.code, server.command.exit.signal)
        return
      }
      if (child.exitCode !== null || child.signalCode !== null) return
      const serving = server.stderr().match(SERVING_LINE)?.[1]
      if (serving !== undefined) {
        cleanup()
        resolveUrl(serving)
      }
    }
    const timeout = setTimeout(() => {
      failed(new Error(`Tola server did not become ready:\n${server.stderr()}${server.stdout()}`))
    }, 15_000)
    child.stderr.on('data', observe)
    child.once('close', closed)
    for (const channel of channels) {
      channel.once('error', failed)
    }
    observe()
  })
}

async function stopServer(command: RunningProcess): Promise<void> {
  if (process.platform === 'win32') {
    await command.terminate()
    return
  }
  if (command.child.exitCode === null && command.child.signalCode === null) {
    command.interrupt()
  }
  try {
    await command.waitForClose(5_000)
  } catch (error) {
    await command.terminate()
    throw error
  }
}

/** Starts the embedded demo itself, without replacing its sources with a scaffold. */
export async function previewDemo(binary: string, id: string, directory: string) {
  const server = startCommand(
    binary,
    ['help', 'demo', id, '--preview', '--no-log-file', '--color', 'never'],
    directory,
  )
  try {
    const url = await waitForServing(server)
    return { ...server, url, close: () => stopServer(server.command) }
  } catch (error) {
    await server.command.terminate()
    throw error
  }
}

type Sites = {
  dev: (options?: SiteOptions) => Promise<RunningSite>
  preview: (options?: SiteOptions) => Promise<RunningSite>
}

export const test = base.extend<{ sites: Sites }>({
  sites: async ({ binary, directory }, use, testInfo) => {
    const starting: Promise<RunningSite>[] = []
    const start = (command: 'dev' | 'preview', options: SiteOptions = {}) => {
      const site = startSite(binary, directory, command, options)
      starting.push(site)
      return site
    }
    let testFailure: unknown
    try {
      await use({
        dev: (options) => start('dev', options),
        preview: (options) => start('preview', options),
      })
    } catch (error) {
      testFailure = error
    }
    const teardownErrors = await closeSites(starting, testInfo)
    if (teardownErrors.length) {
      throw testFailure === undefined
        ? new AggregateError(teardownErrors, 'Site teardown failed')
        : new AggregateError([testFailure, ...teardownErrors], 'Site teardown failed after the test failed')
    }
    if (testFailure !== undefined) throw testFailure
  },
})

/** Closes every started site, attaching failure evidence when the test did not pass. */
async function closeSites(starting: Promise<RunningSite>[], testInfo: TestInfo): Promise<unknown[]> {
  const opened = await Promise.allSettled(starting)
  const closed = await Promise.allSettled(
    opened.map(async (result) => {
      if (result.status === 'rejected') return
      const site = result.value
      try {
        if (testInfo.status !== testInfo.expectedStatus) await attachFailure(site, testInfo)
      } finally {
        await site.close()
      }
    }),
  )
  return closed.flatMap((result) => (result.status === 'rejected' ? [result.reason] : []))
}

async function startSite(
  binary: string,
  directory: string,
  command: 'dev' | 'preview',
  options: SiteOptions,
): Promise<RunningSite> {
  const temporaryRoot = await mkdtemp(join(directory, 'site-'))
  const siteRoot = join(temporaryRoot, 'site')
  const contentRoot = options.contentRoot ?? 'content'
  const watchArguments = options.watch === undefined ? [] : [`--watch=${options.watch}`]
  let activeCommand: RunningCommand | undefined

  try {
    const initialized = await runCommand(
      binary,
      ['init', siteRoot],
      temporaryRoot,
      15_000,
    )
    if (initialized.code !== 0) {
      throw new Error(
        `tola init failed with ${
          initialized.signal ?? `code ${initialized.code}`
        }:\n${initialized.stderr}${initialized.stdout}`,
      )
    }
    const initialContent = options.initialContent === undefined
      ? { relativePath: 'index.typ', source: 'Hello from the process E2E.\n' }
      : options.initialContent
    if (contentRoot !== 'content') await configureContentRoot(siteRoot, contentRoot)
    if (initialContent) {
      await writeContentFile(siteRoot, contentRoot, initialContent.relativePath, initialContent.source)
    }
    if (options.beforeStart) await options.beforeStart(siteRoot)

    const environment: NodeJS.ProcessEnv = { ...options.environment }
    if (options.packageDataRoot) {
      environment.TYPST_PACKAGE_PATH = resolve(siteRoot, options.packageDataRoot)
    }
    if (options.packageCacheRoot) {
      environment.TYPST_PACKAGE_CACHE_PATH = resolve(siteRoot, options.packageCacheRoot)
    }
    let server = startCommand(
      binary,
      [
        command,
        '--interface',
        '127.0.0.1',
        '--port',
        '0',
        ...watchArguments,
        ...(options.inputScope ? [options.inputScope] : []),
      ],
      siteRoot,
      environment,
    )
    activeCommand = server
    const url = await waitForServing(server)
    let closing: Promise<void> | undefined
    return {
      root: siteRoot,
      url,
      get processId() {
        return server.command.child.pid!
      },
      stdout: () => server.stdout(),
      stderr: () => server.stderr(),
      writeContent: (relativePath, source) => writeContentFile(siteRoot, contentRoot, relativePath, source),
      removeContent: (relativePath) => removeContentFile(siteRoot, contentRoot, relativePath),
      restart: async () => {
        if (closing) throw new Error('Cannot restart a closing site')
        await stopServer(server.command)
        server = startCommand(
          binary,
          [
            command,
            '--interface',
            '127.0.0.1',
            '--port',
            new URL(url).port,
            ...watchArguments,
            ...(options.inputScope ? [options.inputScope] : []),
          ],
          siteRoot,
          environment,
        )
        const restartedUrl = await waitForServing(server)
        if (restartedUrl !== url) {
          throw new Error(`Restart changed the site URL from ${url} to ${restartedUrl}`)
        }
      },
      close: () => {
        closing ??= (async () => {
          try {
            await stopServer(server.command)
          } finally {
            await rm(temporaryRoot, { recursive: true, force: true })
          }
        })()
        return closing
      },
    }
  } catch (error) {
    try {
      if (activeCommand) await activeCommand.command.terminate()
    } finally {
      await rm(temporaryRoot, { recursive: true, force: true })
    }
    throw error
  }
}

async function attachFailure(site: RunningSite, testInfo: TestInfo): Promise<void> {
  await testInfo.attach('tola-stdout', { body: site.stdout(), contentType: 'text/plain' })
  await testInfo.attach('tola-stderr', { body: site.stderr(), contentType: 'text/plain' })
  if (process.platform !== 'darwin') return
  const path = testInfo.outputPath('tola-process.sample.txt')
  const sampler = new RunningProcess(
    '/usr/bin/sample',
    [String(site.processId), '1', '-file', path],
    site.root,
  )
  captureOutput(sampler.child)
  sampler.child.stdin.end()
  try {
    const exit = await sampler.waitForClose(5_000)
    if (exit.code !== 0) {
      throw new Error(`Process stack sampling exited with ${exit.signal ?? `code ${exit.code}`}`)
    }
    await testInfo.attach('tola-process-stack', { path, contentType: 'text/plain' })
  } catch (error) {
    await testInfo.attach('tola-process-stack-error', { body: String(error), contentType: 'text/plain' })
  } finally {
    await sampler.terminate()
  }
}

async function writeContentFile(
  siteRoot: string,
  contentRoot: string,
  relativePath: string,
  source: string,
): Promise<void> {
  const contentPath = resolveSiteRelative(join(siteRoot, contentRoot), relativePath, 'content path')
  await mkdir(dirname(contentPath), { recursive: true })
  await writeFile(contentPath, source)
}

async function configureContentRoot(siteRoot: string, contentRoot: string): Promise<void> {
  resolveSiteRelative(siteRoot, contentRoot, 'content root')
  const configPath = join(siteRoot, 'tola.toml')
  const source = await readFile(configPath, 'utf8')
  const updated = source.replace(
    /^content-dir = "content"$/m,
    `content-dir = ${JSON.stringify(contentRoot)}`,
  )
  if (updated === source) throw new Error('could not update the generated content root')
  await writeFile(configPath, updated)
  await mkdir(join(siteRoot, contentRoot), { recursive: true })
}

function resolveSiteRelative(root: string, path: string, label: string): string {
  if (path.length === 0 || isAbsolute(path)) throw new Error(`${label} must be site-relative: ${path}`)
  const absoluteRoot = resolve(root)
  const absolute = resolve(absoluteRoot, path)
  const fromRoot = relative(absoluteRoot, absolute)
  if (
    fromRoot === '..' || fromRoot.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) ||
    isAbsolute(fromRoot)
  ) {
    throw new Error(`${label} escapes its root: ${path}`)
  }
  return absolute
}

async function removeContentFile(
  siteRoot: string,
  contentRoot: string,
  relativePath: string,
): Promise<void> {
  await rm(resolveSiteRelative(join(siteRoot, contentRoot), relativePath, 'content path'), { force: true })
}
