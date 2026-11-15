import { type ChildProcess, spawn } from 'node:child_process'
import * as vscode from 'vscode'

import { type BuildDiagnostic, type BuildDiagnostics, buildErrorMessage, savedDocuments } from './build-diagnostics.ts'
import { type Selection, siteArguments } from './selection.ts'

/** The site's own CLI verbs the editor drives, with their output on the Tola channel; their children die with the extension. */
export class SiteCli {
  private readonly children = new Set<ChildProcess>()
  private readonly building = new Map<string, ChildProcess>()

  constructor(
    private readonly output: vscode.OutputChannel,
    private readonly report: (error: unknown) => void,
    private readonly diagnostics: BuildDiagnostics,
  ) {}

  register(): vscode.Disposable {
    return {
      dispose: () => {
        for (const child of this.children) {
          child.kill('SIGINT')
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
    const args = [...siteArguments(selection, 'build'), '--json']
    const reports = this.diagnostics.open(selection, 'build')
    const snapshot = savedDocuments()
    const chunks: Buffer[] = []
    this.output.appendLine(`Tola builds ${selection.configurationFile}`)
    const started = Date.now()
    const child = this.start(selection.command, args, selection.folder.uri.fsPath, (chunk) => chunks.push(chunk))
    this.building.set(key, child)
    const completed = new Promise<void>((resolve) => {
      let failedToStart = false
      const finish = () => {
        this.building.delete(key)
        resolve()
      }
      child.once('error', () => {
        failedToStart = true
        reports.clear()
        this.report(
          new Error(`Tola could not start the build for ${selection.folder.name}.`),
        )
        finish()
      })
      child.once('close', (code, signal) => {
        if (failedToStart) return
        const output = Buffer.concat(chunks).toString('utf8')
        let report: { diagnostics: readonly BuildDiagnostic[]; cancelled: boolean } | undefined
        try {
          const parsed = JSON.parse(output)
          if (parsed?.formatVersion !== 1 || parsed.operation !== 'build' || !Array.isArray(parsed.diagnostics)) {
            throw new Error('Unsupported build report format')
          }
          report = parsed
          if (parsed.cancelled) reports.clear()
          else reports.set(parsed.diagnostics, snapshot)
        } catch (error) {
          reports.clear()
          if (output.trim()) this.output.append(output)
          this.output.appendLine(`Tola did not return a build report: ${String(error)}`)
        }
        const seconds = ((Date.now() - started) / 1000).toFixed(1)
        const error = buildErrorMessage(report?.diagnostics ?? [])
        if (code === 0) {
          this.output.appendLine(`Tola built ${selection.folder.name} in ${seconds}s.`)
        } else if (!report?.cancelled) {
          this.report(
            new Error(
              `Tola could not build ${selection.folder.name}. ${
                error ?? `Open Tola output for details (${signal ?? `exit ${code ?? 'unknown'}`}).`
              }`,
            ),
          )
        }
        finish()
      })
    })
    await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Window, title: `Tola: Building ${selection.folder.name}` },
      () => completed,
    )
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
    const args = [...siteArguments(selection, 'inspect'), 'config', '--json']
    let printed = ''
    const child = this.start(selection.command, args, selection.folder.uri.fsPath, (chunk) => {
      printed += chunk.toString()
    })
    const code = await new Promise<number | null>((resolve, reject) => {
      child.once('error', () => {
        reject(new Error(`Tola could not read the configuration of ${selection.folder.name}.`))
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
      this.output.append(printed)
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

  /** Merge workspace settings and optionally configure the official Nushell extension. */
  async editorSetup(selection: Selection): Promise<void> {
    const nushell = 'Set Up with Nushell'
    const options = process.platform !== 'win32' && vscode.workspace.workspaceFolders?.length === 1
      ? ['Set Up Integration', nushell]
      : ['Set Up Integration']
    const selected = await vscode.window.showWarningMessage(
      `Set up the editor integration for ${selection.folder.name}?`,
      { detail: 'Tola updates workspace settings and package sources. Choose Set Up with Nushell to have the Nushell extension use Tola for language features.' },
      ...options,
    )
    if (!selected) return
    const args = ['editor', ...siteArguments(selection, 'setup'), 'vscode']
    if (selected === nushell) args.push('--nu')
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
      ['init', directory.fsPath],
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
    const completed = new Promise<{ code: number | null; error?: Error }>((resolve) => {
      child.once('error', (error: Error) => resolve({ code: null, error }))
      child.once('close', (code: number | null) => resolve({ code }))
    })
    const outcome = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Window, title: `Tola: ${label}` },
      () => completed,
    )
    if (outcome.error) {
      this.report(new Error(`Tola could not start ${label}. Open Tola output for details.`))
    } else if (outcome.code !== 0) {
      this.report(
        new Error(
          `Tola could not run ${label} in ${cwd} (exit ${
            outcome.code ?? 'unknown'
          }); the Tola output channel holds its diagnostics.`,
        ),
      )
    }
  }

  private start(
    command: string,
    args: string[],
    cwd: string,
    stdout: (chunk: Buffer) => void = (chunk) => this.output.append(chunk.toString()),
  ): ChildProcess {
    const child = spawn(command, args, {
      cwd,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    this.children.add(child)
    child.once('error', (error: Error) => this.output.appendLine(error.message))
    child.stdout?.on('data', stdout)
    child.stderr?.on('data', (chunk: Buffer) => this.output.append(chunk.toString()))
    child.once('close', () => this.children.delete(child))
    return child
  }
}
