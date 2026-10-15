import { expect } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { commandRunner, expectExited, test } from '../../support/process.ts'
import { writeMinimalSite } from '../../support/site.ts'

for (const flag of ['--pure', '--ignore-system-fonts']) {
  test(`a font listing narrowed by ${flag} carries no system fonts`, async ({ binary, directory: root }) => {
    const run = commandRunner(binary)
    await writeMinimalSite(root)
    await writeFile(join(root, 'tola.toml'), '[typst.fonts]\nsystem = true\n')

    const result = await run(['fonts', flag], root)
    expectExited(result)
    const lines = result.stdout.trim().split('\n')
    expect(lines.length).toBeGreaterThan(0)
    expect(lines.every((line) => line.includes('embedded'))).toBe(true)
  })
}

test('a pure listing refuses host font directories', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)

  const result = await run(['fonts', '--pure', '--font-path', root], root)
  expectExited(result, 1)
  expect(result.stderr).toContain('--font-path')
})

test('font records carry the face and its file', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)

  const result = await run(['fonts', '--json', '--pure'], root)
  expectExited(result)
  const records = JSON.parse(result.stdout) as Record<string, unknown>[]
  expect(records.length).toBeGreaterThan(0)
  expect(
    records.every(
      (record) =>
        typeof record.family === 'string' &&
        typeof record.style === 'string' &&
        typeof record.weight === 'number' &&
        typeof record.stretch === 'string' &&
        record.file === null,
    ),
  ).toBe(true)
})

test('a workspace without a configuration still lists fonts', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)

  const result = await run(['fonts'], root)
  expectExited(result)
  expect(result.stdout.trim().length).toBeGreaterThan(0)
})
