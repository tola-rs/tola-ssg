import { expect } from '@playwright/test'
import { stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { COMMAND_TIMEOUT_MS, commandRunner, expectExited, test } from '../../support/process.ts'
import { writeMinimalSite } from '../../support/site.ts'

test('check --no-build reports sources without building', async ({ binary, directory: root }) => {
  const run = commandRunner(binary)
  await writeMinimalSite(root)
  const broken = join(root, 'content/broken.typ')
  await writeFile(broken, '= Title\n\n#let = \n')

  const failed = await run(['check', '--no-build'], root, COMMAND_TIMEOUT_MS.standard)
  expectExited(failed, 1)
  expect(failed.stderr).toContain('content/broken.typ')
  expect(failed.stderr).toContain('does not parse as Typst')

  await writeFile(broken, '#let x = 1\n')
  expectExited(await run(['check', '--no-build'], root, COMMAND_TIMEOUT_MS.standard))
  await expect(stat(join(root, 'public'))).rejects.toThrow()
})
