import { type ChildProcess, spawn } from 'node:child_process'
import * as vscode from 'vscode'

import { type Selection, siteArguments } from './selection.ts'

/** The site's own CLI verbs the editor drives, with their output on the Tola channel; their children die with the extension. */
export class SiteCli {
  private readonly children = new Set<ChildProcess>()
  private readonly building = new Map<string, ChildProcess>()

  constructor(
    private readonly output: vscode.OutputChannel,
    private readonly report: (error: unknown) => void,
  ) {}

  register(): vscode.Disposable {
    return {
      dispose: () => {
        for (const child of this.children) {
          child.kill('SIGINT')
          const timer = setTimeout(() => child.kill('SIGKILL'), 5_000)
          timer.unref()
        }
      },
    }
  }

  async build(selection: Selection): Promise<void> {
    const key = selection.folder.uri.toString()
    if (this.building.has(key)) {
      void vscode.window.showInformationMessage(`Tola is already building ${selection.folder.name}.`)
      return
    }
    const args = siteArguments(selection, 'build')
    this.output.appendLine(`Tola builds ${selection.configurationFile}`)
    const started = Date.now()
    const child = this.start(selection.command, args, selection.folder.uri.fsPath)
    this.building.set(key, child)
    await new Promise<void>((resolve) => {
      const finish = () => {
        this.building.delete(key)
        resolve()
      }
      child.once('error', (error) => {
        this.report(
          new Error(`Tola could not start the build for ${selection.folder.name}: ${error.message}`),
        )
        finish()
      })
      child.once('close', (code, signal) => {
        const seconds = ((Date.now() - started) / 1000).toFixed(1)
        if (code === 0) {
          this.output.appendLine(`Tola built ${selection.folder.name} in ${seconds}s.`)
          void vscode.window.showInformationMessage(`Tola built ${selection.folder.name}.`)
        } else {
          this.report(
            new Error(
              `Tola could not build ${selection.folder.name} (${
                signal ?? `exit ${code ?? 'unknown'}`
              }); the Tola output channel holds its diagnostics.`,
            ),
          )
        }
        finish()
      })
    })
  }

  async revealOutput(selection: Selection): Promise<void> {
    const directory = await this.outputDirectory(selection)
    const uri = vscode.Uri.file(directory)
    const exists = await vscode.workspace.fs.stat(uri).then(() => true, () => false)
    if (!exists) {
      this.output.appendLine(`Tola has not built ${directory} yet; run Tola: Build Site first.`)
      void vscode.window.showInformationMessage('Tola has not built this site yet; run Tola: Build Site.')
      return
    }
    await vscode.commands.executeCommand('revealFileInOS', uri)
  }

  private async outputDirectory(selection: Selection): Promise<string> {
    const args = ['config', ...selection.args.slice(1)]
    const child = this.start(selection.command, args, selection.folder.uri.fsPath)
    let printed = ''
    child.stdout?.on('data', (chunk: Buffer) => {
      printed += chunk.toString()
    })
    child.stderr?.on('data', (chunk: Buffer) => this.output.append(chunk.toString()))
    const code = await new Promise<number | null>((resolve) => {
      child.once('error', (error) => {
        this.report(
          new Error(`Tola could not read the configuration of ${selection.folder.name}: ${error.message}`),
        )
        resolve(null)
      })
      child.once('close', resolve)
    })
    if (code !== 0) {
      throw new Error(
        `Tola could not read the configuration of ${selection.folder.name} (exit ${
          code ?? 'unknown'
        }); the Tola output channel holds its diagnostics.`,
      )
    }
    let resolved: unknown
    try {
      resolved = JSON.parse(printed)
    } catch {
      throw new Error("Tola's configuration did not print JSON; the Tola output channel holds its output")
    }
    const directory = (resolved as { output?: unknown }).output
    if (typeof directory !== 'string' || !directory) {
      throw new Error("Tola's configuration reported no output directory")
    }
    return directory
  }

  /** `tola vendor`: freezes the packages the site built with into the site's own vendor directory. */
  async vendorPackages(selection: Selection): Promise<void> {
    if (
      !await this.confirm(
        `Vendor the packages ${selection.folder.name} built with?`,
        `Tola writes the frozen packages beside ${selection.configurationFile} and points later builds at that copy.`,
        'Vendor Packages',
      )
    ) return
    await this.run(
      selection.command,
      siteArguments(selection, 'vendor'),
      selection.folder.uri.fsPath,
      'Vendor Packages',
    )
  }

  /** `tola editor setup vscode`: regenerates this editor's package mirror and merges its settings. */
  async editorSetup(selection: Selection): Promise<void> {
    if (
      !await this.confirm(
        `Set up the editor integration for ${selection.folder.name}?`,
        'Tola writes its package mirror for editors and merges the workspace settings the integration needs.',
        'Set Up Integration',
      )
    ) return
    const args = [...siteArguments(selection, 'editor'), 'setup', 'vscode']
    await this.run(selection.command, args, selection.folder.uri.fsPath, 'Set Up Editor Integration')
  }

  /** `tola init`: writes a starting site in the directory the author picked and confirmed. */
  async initSite(command: string, directory: vscode.Uri): Promise<void> {
    if (
      !await this.confirm(
        `Create a Tola site in ${directory.fsPath}?`,
        'Tola writes its starting configuration and content there; existing files are never overwritten.',
        'Create Site',
      )
    ) return
    await this.run(
      command,
      ['init', directory.fsPath, '--no-interactive', '--editor', 'vscode'],
      directory.fsPath,
      'Create Site',
    )
  }

  /** The author's decision before a verb writes into the workspace: dismissing it decides nothing. */
  private async confirm(question: string, detail: string, button: string): Promise<boolean> {
    return await vscode.window.showWarningMessage(question, { detail }, button) === button
  }

  /** Run one verb and report its exit status the way `build` reports its own. */
  private async run(command: string, args: string[], cwd: string, label: string): Promise<void> {
    this.output.appendLine(`Tola runs ${label} in ${cwd}`)
    const child = this.start(command, args, cwd)
    const outcome = await new Promise<{ code: number | null; error?: Error }>((resolve) => {
      child.once('error', (error: Error) => resolve({ code: null, error }))
      child.once('close', (code: number | null) => resolve({ code }))
    })
    if (outcome.error) {
      this.report(new Error(`Tola could not start ${label} in ${cwd}: ${outcome.error.message}`))
    } else if (outcome.code === 0) {
      void vscode.window.showInformationMessage(`Tola ran ${label} in ${cwd}.`)
    } else {
      this.report(
        new Error(
          `Tola could not run ${label} in ${cwd} (exit ${
            outcome.code ?? 'unknown'
          }); the Tola output channel holds its diagnostics.`,
        ),
      )
    }
  }

  private start(command: string, args: string[], cwd: string): ChildProcess {
    const child = spawn(command, args, {
      cwd,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    this.children.add(child)
    child.stdout?.on('data', (chunk: Buffer) => this.output.append(chunk.toString()))
    child.stderr?.on('data', (chunk: Buffer) => this.output.append(chunk.toString()))
    child.once('close', () => this.children.delete(child))
    return child
  }
}
