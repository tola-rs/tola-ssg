import { expect } from '@playwright/test'
import { mkdir, readFile, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { LanguageConnection } from '../../support/lsp.ts'
import {
  captureOutput,
  COMMAND_TIMEOUT_MS,
  expectExited,
  runCommand,
  RunningProcess,
  startCommand,
  test,
} from '../../support/process.ts'

test('Nu forwards script arguments and help without a site', async ({ binary, directory: root }) => {
  await writeFile(
    join(root, 'hello.nu'),
    '# Greet someone\ndef main [name: string, --color: string] { [$name $color] | to json --raw }\n',
  )
  const run = await runCommand(
    binary,
    ['nu', 'hello.nu', 'a name', '--color', 'blue'],
    root,
    COMMAND_TIMEOUT_MS.short,
  )
  expectExited(run)
  expect(JSON.parse(run.stdout)).toEqual(['a name', 'blue'])
  const help = await runCommand(binary, ['nu', 'hello.nu', '--help'], root, COMMAND_TIMEOUT_MS.short)
  expectExited(help)
  expect(help.stdout).toContain('Greet someone')
})

test('Nu forwards stdin and exit status', async ({ binary, directory: root }) => {
  await writeFile(join(root, 'input.nu'), 'def main [] { $in | str upcase }\n')
  const command = new RunningProcess(binary, ['nu', '--stdin', 'input.nu'], root)
  const output = captureOutput(command.child)
  try {
    command.child.stdin.end('hello\n')
    expectExited({ ...await command.waitForClose(COMMAND_TIMEOUT_MS.short), stderr: output.stderr() })
    expect(output.stdout().trim()).toBe('HELLO')
  } finally {
    await command.terminate()
  }
  await writeFile(join(root, 'fail.nu'), 'exit 7\n')
  expectExited(await runCommand(binary, ['nu', 'fail.nu'], root, COMMAND_TIMEOUT_MS.short), 7)
})

test('registered Nu commands retain module and working directory', async ({ binary, directory: root }) => {
  const module = join(root, 'theme {project}')
  await mkdir(join(module, 'tools'), { recursive: true })
  await mkdir(join(root, 'empty-path'))
  await writeFile(
    join(root, 'tola.toml'),
    "[modules.notes]\npath = 'theme {project}'\n[build]\ncontent-dir = 'my notes'\n",
  )
  await writeFile(
    join(module, 'tola-module.toml'),
    `schema-version = 1
[commands.new]
help = "Create a note"
kind = "argv"
argv = ["{tola}", "nu", "tools/new.nu"]
`,
  )
  await writeFile(
    join(module, 'tools/new.nu'),
    `def main [title: string] {
    {title: $title, module: $env.TOLA_MODULE_NAME, content: $env.TOLA_CONTENT_ROOT,
     executable: $env.TOLA_EXECUTABLE, script: $env.CURRENT_FILE, cwd: $env.PWD} | to json
}
`,
  )
  const command = startCommand(binary, ['notes', 'new', 'A title {module}; with spaces'], root, {
    PATH: join(root, 'empty-path'),
  })
  try {
    expectExited({
      ...await command.command.waitForClose(COMMAND_TIMEOUT_MS.short),
      stderr: command.stderr(),
    })
    const value = JSON.parse(command.stdout())
    expect(value.title).toBe('A title {module}; with spaces')
    expect(value.module).toBe('notes')
    expect(value.content).toBe(join(await realpath(root), 'my notes'))
    expect(await realpath(value.executable)).toBe(await realpath(binary))
    expect(await realpath(value.script)).toBe(await realpath(join(module, 'tools/new.nu')))
    expect(await realpath(value.cwd)).toBe(await realpath(root))
  } finally {
    await command.command.terminate()
  }
})

test('Nu imports bundled commands without personal configuration', async ({ binary, directory: root }) => {
  await mkdir(join(root, 'tools'))
  await mkdir(join(root, 'lib'))
  await mkdir(join(root, 'personal/nushell'), { recursive: true })
  await writeFile(join(root, 'personal/nushell/config.nu'), "error make {msg: 'personal config loaded'}")
  await writeFile(join(root, 'lib/labels.nu'), "export def label [] { 'second' }\n")
  await writeFile(
    join(root, 'tools/select.nu'),
    `use ../lib/labels.nu label
use std/assert
def main [] {
  assert equal (label) 'second'
  [{name: first}, {name: (label)}] | tui table | tui debug --keys [down enter] | get selected.name
}
`,
  )
  const command = startCommand(binary, ['nu', 'tools/select.nu'], root, {
    XDG_CONFIG_HOME: join(root, 'personal'),
  })
  try {
    expectExited({
      ...await command.command.waitForClose(COMMAND_TIMEOUT_MS.short),
      stderr: command.stderr(),
    })
    expect(command.stdout().trim()).toBe('second')
  } finally {
    await command.command.terminate()
  }
})

test('Nu language server answers unsaved sources', async ({ binary, directory: root }) => {
  const connection = new LanguageConnection(binary, root, {}, ['nu', '--lsp'])
  const uri = pathToFileURL(join(root, 'example.nu')).href
  try {
    await connection.initialize({})
    connection.send({
      method: 'textDocument/didOpen',
      params: { textDocument: { uri, languageId: 'nu', version: 1, text: '"hello" | str upcase\n' } },
    })
    const hover = await connection.sendRequest<{ contents: unknown }>('textDocument/hover', {
      textDocument: { uri },
      position: { line: 0, character: 12 },
    })
    expect(hover.contents).toBeTruthy()
    await connection.close()
  } finally {
    await connection.terminate()
  }
})

test('module hooks build through the embedded Nu runtime', async ({ binary, directory: root }) => {
  await mkdir(join(root, 'theme/tools'), { recursive: true })
  await mkdir(join(root, 'content'))
  await mkdir(join(root, 'empty-path'))
  await writeFile(join(root, 'tola.toml'), '[modules.notes]\npath = "theme"\n[typst.fonts]\nsystem = false\n')
  await writeFile(
    join(root, 'theme/tola-module.toml'),
    `schema-version = 1
[[hooks]]
stage = "before-build"
name = "prepare"
command = ["{tola}", "nu", "tools/prepare.nu"]
generates = ["generated/{name}/context.json"]
`,
  )
  await writeFile(
    join(root, 'theme/tools/prepare.nu'),
    `def main [] {
  let output = [$env.TOLA_PROJECT_ROOT generated $env.TOLA_MODULE_NAME] | path join
  mkdir $output
  {scope: $env.TOLA_INPUT_SCOPE, stage: $env.TOLA_HOOK_STAGE,
   module: $env.TOLA_MODULE_NAME, options: (open $env.TOLA_MODULE_OPTIONS)}
  | to json | save ($output | path join context.json)
}
`,
  )
  await writeFile(
    join(root, 'site.typ'),
    '#let settings = json("generated/notes/context.json")\n#document("index.html", format: "html")[#settings.scope / #settings.module]\n',
  )
  const command = startCommand(binary, ['build', '--pure'], root, { PATH: join(root, 'empty-path') })
  try {
    expectExited({
      ...await command.command.waitForClose(COMMAND_TIMEOUT_MS.build),
      stderr: command.stderr(),
    })
    expect(JSON.parse(await readFile(join(root, 'generated/notes/context.json'), 'utf8')))
      .toEqual({ scope: 'pure', stage: 'before-build', module: 'notes', options: {} })
    expect(await readFile(join(root, 'public/index.html'), 'utf8')).toContain('pure / notes')
  } finally {
    await command.command.terminate()
  }
})
