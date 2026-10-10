import { realpath } from 'node:fs/promises'
import * as path from 'node:path'
import * as vscode from 'vscode'

/** The settings that add a source root to the site, and the CLI flag each is passed as. */
const PACKAGE_ROOT_ARGUMENTS: readonly (readonly [string, string])[] = [
  ['packagePath', '--package-path'],
  ['packageCachePath', '--package-cache-path'],
]

interface WorkspaceSelection {
  folder: vscode.WorkspaceFolder
  command: string
  args: string[]
  inputRoots: readonly string[]
}

export interface Selection extends WorkspaceSelection {
  kind: 'site'
  config: vscode.Uri
  configurationFile: string
}

export interface DocumentsSelection extends WorkspaceSelection {
  kind: 'documents'
}

export type ServedWorkspace = Selection | DocumentsSelection

function configuredPath(folder: vscode.WorkspaceFolder, value: string): string {
  return path.resolve(folder.uri.fsPath, value.replaceAll('${workspaceFolder}', folder.uri.fsPath))
}

export function containsFile(root: string, filename: string): boolean {
  const relative = path.relative(root, filename)
  return relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative)
}

export function ownsTypstDocument(selected: ServedWorkspace, document: vscode.TextDocument): boolean {
  return document.uri.scheme === 'file' && document.languageId === 'typst' &&
    vscode.workspace.getWorkspaceFolder(document.uri)?.uri.toString() === selected.folder.uri.toString()
}

/** An unsaved Typst buffer: no file on disk, and language features answer from the one site that owns it. */
export function isUntitledTypst(document: vscode.TextDocument): boolean {
  return document.uri.scheme === 'untitled' && document.languageId === 'typst'
}

/** A site's configuration answers by both its selected path and its resolved path. */
export function ownsDocument(selected: ServedWorkspace, document: vscode.TextDocument): boolean {
  return ownsTypstDocument(selected, document) || selected.kind === 'site' &&
      document.uri.scheme === 'file' &&
      (document.uri.fsPath === selected.config.fsPath || document.uri.fsPath === selected.configurationFile)
}

/** The configured path is authoritative even before the file exists. */
export function configurationUri(folder: vscode.WorkspaceFolder): vscode.Uri | undefined {
  if (!vscode.workspace.isTrusted || folder.uri.scheme !== 'file') return undefined
  const settings = vscode.workspace.getConfiguration('tola', folder.uri)
  if (!settings.get<boolean>('enabled', true)) return undefined
  const selected = settings.get<string>('configPath', 'tola.toml')
  if (!selected.trim()) {
    throw new Error(
      `the \`tola.configPath\` setting is empty in ${folder.name}; set it to the path of the site's \`tola.toml\``,
    )
  }
  return vscode.Uri.file(configuredPath(folder, selected))
}

export async function selectSite(
  folder: vscode.WorkspaceFolder,
  config: vscode.Uri,
): Promise<Selection | undefined> {
  let exists = true
  try {
    const stat = await vscode.workspace.fs.stat(config)
    if (!(stat.type & vscode.FileType.File)) {
      throw new Error(
        `the \`tola.configPath\` setting in ${folder.name} does not point to a file; set it to the path of the site's \`tola.toml\``,
      )
    }
  } catch (error) {
    if (!(error instanceof vscode.FileSystemError && error.code === 'FileNotFound')) throw error
    const configured = vscode.workspace.getConfiguration('tola', folder.uri).get<string>(
      'configPath',
      'tola.toml',
    )
    if (configured === 'tola.toml') return undefined
    exists = false
  }
  const [workspaceRoot, configurationRoot, configurationFile] = exists
    ? await Promise.all([
      realpath(folder.uri.fsPath),
      realpath(path.dirname(config.fsPath)),
      realpath(config.fsPath),
    ])
    : [folder.uri.fsPath, path.dirname(config.fsPath), config.fsPath]
  // The site root is the configuration's directory; everything the server mirrors lives under it,
  // plus whatever package directories the settings name.
  const sourceRoot = containsFile(workspaceRoot, configurationRoot)
    ? path.join(folder.uri.fsPath, path.relative(workspaceRoot, configurationRoot))
    : path.dirname(config.fsPath)
  // Route-index source paths resolve against the first root; configured package roots follow it.
  return {
    ...selectWorkspace(folder, sourceRoot, ['lsp', '--config', config.fsPath]),
    kind: 'site',
    config,
    configurationFile,
  }
}

export function selectDocuments(folder: vscode.WorkspaceFolder): DocumentsSelection {
  return { ...selectWorkspace(folder, folder.uri.fsPath, ['lsp']), kind: 'documents' }
}

function selectWorkspace(
  folder: vscode.WorkspaceFolder,
  sourceRoot: string,
  args: string[],
): WorkspaceSelection {
  const roots = new Set([sourceRoot])
  const settings = vscode.workspace.getConfiguration('tola', folder.uri)
  const command = serverCommand(folder.uri, folder.name)
  for (const [setting, flag] of PACKAGE_ROOT_ARGUMENTS) {
    const value = settings.get<string>(setting, '')
    if (value) {
      const directory = configuredPath(folder, value)
      args.push(flag, directory)
      roots.add(directory)
    }
  }
  return { folder, command, args, inputRoots: [...roots] }
}

/** The CLI arguments that run one verb against the site a selection names, reusing the site's flags. */
export function siteArguments(selected: Selection, verb: string): string[] {
  return [verb, ...selected.args.slice(1)]
}

/**
 * The Tola executable a folder's `tola.serverPath` names, as a command the editor can spawn.
 *
 * A configured path is authoritative, and `${workspaceFolder}` inside it names the folder itself;
 * a bare name is left for the shell to resolve.
 */
export function serverCommand(resource: vscode.Uri, label: string): string {
  const executable = vscode.workspace.getConfiguration('tola', resource).get<string>('serverPath', 'tola')
  if (!executable.trim()) {
    throw new Error(
      `the \`tola.serverPath\` setting is empty in ${label}; set it to the command name or path of the Tola executable`,
    )
  }
  return /[/\\]/.test(executable)
    ? path.resolve(resource.fsPath, executable.replaceAll('${workspaceFolder}', resource.fsPath))
    : executable
}

export function sameSelection(left: ServedWorkspace, right: ServedWorkspace): boolean {
  return left.kind === right.kind && left.command === right.command &&
    (left.kind !== 'site' || right.kind === 'site' && left.configurationFile === right.configurationFile) &&
    left.args.length === right.args.length &&
    left.args.every((argument, index) => argument === right.args[index])
}
