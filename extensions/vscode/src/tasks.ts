import * as vscode from 'vscode'

import { type Selection, siteArguments } from './selection.ts'

/** One Tola CLI verb of one site, as a task the editor can run and re-run. */
export type TolaTaskDefinition = vscode.TaskDefinition & {
  verb: string
  /** The workspace folder whose site the task runs in; absent uses the folder the task starts in. */
  site?: string
}

/**
 * The `Tola: Build` task, one per enabled site: the same CLI invocation the `tola.build` command
 * makes, so a task and the command can never drift apart.
 *
 * No problem matcher is declared: the CLI prints its diagnostics as one pictographic report, the
 * language client already publishes the positioned diagnostics the editor shows, and a matcher over
 * the report would report every failure twice.
 */
export class SiteTasks implements vscode.TaskProvider {
  constructor(private readonly selections: () => readonly Selection[]) {}

  register(): vscode.Disposable {
    return vscode.tasks.registerTaskProvider('tola', this)
  }

  provideTasks(): vscode.Task[] {
    return this.selections()
      .filter((selection) => selection.folder.uri.scheme === 'file')
      .map((selection) => this.task(selection, { type: 'tola', verb: 'build' }))
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const definition = task.definition as TolaTaskDefinition
    if (definition.verb !== 'build') return undefined
    const scope = task.scope
    const candidates = this.selections().filter((candidate) =>
      definition.site !== undefined
        ? candidate.folder.name === definition.site
        : typeof scope === 'object'
        ? candidate.folder.uri.toString() === scope.uri.toString()
        : scope === vscode.TaskScope.Workspace
    )
    const [selection] = candidates
    return selection && candidates.length === 1 ? this.task(selection, definition) : undefined
  }

  private task(selection: Selection, definition: TolaTaskDefinition): vscode.Task {
    const execution = new vscode.ProcessExecution(
      selection.command,
      siteArguments(selection, 'build'),
      { cwd: selection.folder.uri.fsPath },
    )
    return new vscode.Task(
      { ...definition, site: selection.folder.name },
      selection.folder,
      `Tola: Build ${selection.folder.name}`,
      'tola',
      execution,
    )
  }
}
