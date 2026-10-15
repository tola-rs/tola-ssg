import { expect } from '@playwright/test'
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { COMMAND_TIMEOUT_MS, commandRunner, expectExited, test } from '../../support/process.ts'
import { writeMinimalSite } from '../../support/site.ts'

test('fmt formats every source the site owns', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  const source = join(root, 'content/post.typ')
  const template = join(root, 'templates/card.typ')
  await mkdir(join(root, 'templates'), { recursive: true })
  await writeFile(source, '#let  x =  1\n')
  await writeFile(template, '#let  name =  "card"\n')

  expectExited(await run(['fmt', '--check'], root, COMMAND_TIMEOUT_MS.standard), 1)
  expect(await readFile(source, 'utf8')).toBe('#let  x =  1\n')

  expectExited(await run(['fmt'], root, COMMAND_TIMEOUT_MS.standard))
  expect(await readFile(source, 'utf8')).toBe('#let x = 1\n')
  expect(await readFile(template, 'utf8')).toBe('#let name = "card"\n')

  expectExited(await run(['fmt', '--check'], root, COMMAND_TIMEOUT_MS.standard))
})

test('fmt formats a directory with no site configuration', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  const source = join(root, 'draft.typ')
  await writeFile(source, '#let  x =  1\n')

  expectExited(await run(['fmt'], root, COMMAND_TIMEOUT_MS.standard))
  expect(await readFile(source, 'utf8')).toBe('#let x = 1\n')
  expectExited(await run(['fmt', '--check'], root, COMMAND_TIMEOUT_MS.standard))

  await expect(stat(join(root, '.tola'))).rejects.toThrow()
})
