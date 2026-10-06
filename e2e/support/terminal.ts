import { COMMAND_TIMEOUT_MS, type FinishedCommand, startCommand } from './process.ts'

/** Single-quotes one argv word for the shell Linux `script -c` runs. */
export function shellQuote(word: string): string {
  return `'${word.replaceAll("'", "'\\''")}'`
}

/** The rows a pseudo-terminal is given by [`sizedTerminalArgs`]. */
export const PTY_ROWS = 24

/** The columns a pseudo-terminal is given by [`sizedTerminalArgs`]. */
export const PTY_COLUMNS = 80

/** `script` arguments that run `args` on a pseudo-terminal whose size the test sets. */
export function sizedTerminalArgs(binary: string, args: string[]): string[] {
  const command = [binary, ...args].map(shellQuote).join(' ')
  const sized = `stty rows ${PTY_ROWS} cols ${PTY_COLUMNS}; exec ${command}`
  return process.platform === 'darwin'
    ? ['-q', '/dev/null', 'sh', '-c', sized]
    : ['-q', '-e', '-c', sized, '/dev/null']
}

/** Runs the binary with stdout attached to a pseudo-terminal through `script`. */
export async function runOnTerminal(
  binary: string,
  args: string[],
  root: string,
  environment: NodeJS.ProcessEnv,
): Promise<FinishedCommand> {
  const argv = [binary, ...args]
  const scriptArgs = process.platform === 'darwin'
    ? ['-q', '/dev/null', ...argv]
    : ['-q', '-e', '-c', argv.map(shellQuote).join(' '), '/dev/null']
  const running = startCommand('script', scriptArgs, root, environment)
  const exit = await running.command.waitForClose(COMMAND_TIMEOUT_MS.standard)
  return { ...exit, stdout: running.stdout(), stderr: running.stderr() }
}

/**
 * The text a terminal shows after replaying `bytes` over a `rows`×`columns` window.
 *
 * A frame is written as cursor-addressed runs, and a cell that already shows its final character
 * is not written at all, so reading the stream without the cursor would run a row's words
 * together and lose the text an unchanged cell still shows.
 */
const WIDE =
  /[\u1100-\u115f\u2e80-\u303e\u3041-\u33ff\u3400-\u4dbf\u4e00-\u9fff\ua000-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe6f\uff00-\uff60\uffe0-\uffe6]/u

/** Whether one symbol occupies two terminals cells: the East Asian wide ranges held here. */
function isWide(character: string): boolean {
  return WIDE.test(character)
}

export function screenText(
  bytes: Uint8Array | string,
  { rows = PTY_ROWS, columns = PTY_COLUMNS }: { rows?: number; columns?: number } = {},
): string {
  const screen = Array.from({ length: rows }, () => Array<string>(columns).fill(' '))
  const source = typeof bytes === 'string' ? bytes : new TextDecoder().decode(bytes)
  let row = 0
  let column = 0
  let index = 0
  while (index < source.length) {
    if (source[index] === '\x1b') {
      if (source[index + 1] === ']') {
        // An OSC payload (a window title) is consumed whole, up to BEL or ST.
        const end = source.indexOf('\x07', index)
        const terminator = source.indexOf('\x1b\\', index)
        const stop = end === -1 ? terminator : terminator === -1 ? end : Math.min(end, terminator)
        index = stop === -1 ? source.length : stop + (stop === end ? 1 : 2)
        continue
      }
      if (source[index + 1] !== '[') {
        index += 1
        continue
      }
      let cursor = index + 2
      while (cursor < source.length && (source[cursor] ?? '') < '@') cursor += 1
      const final = source[cursor]
      if (final === undefined || final > '~') {
        index += 1
        continue
      }
      const parameters = source.slice(index + 2, cursor)
      const [first = Number.NaN, second = Number.NaN] = parameters.split(';').map(Number)
      const steps = first > 0 ? first : 1
      index = cursor + 1
      switch (final) {
        case 'H':
        case 'f':
          row = Math.min(first > 0 ? first : 1, rows) - 1
          column = Math.min(second > 0 ? second : 1, columns) - 1
          break
        case 'A':
          row = Math.max(row - steps, 0)
          break
        case 'B':
          row = Math.min(row + steps, rows - 1)
          break
        case 'C':
          column = Math.min(column + steps, columns - 1)
          break
        case 'D':
          column = Math.max(column - steps, 0)
          break
        case 'G':
          column = Math.min(first > 0 ? first : 1, columns) - 1
          break
        case 'J':
          if (first === 2 || first === 3) screen.forEach((line) => line.fill(' '))
          else if (!(first > 0)) {
            screen[row]?.fill(' ', column)
            for (let line = row + 1; line < rows; line += 1) screen[line]?.fill(' ')
          }
          break
        case 'K':
          if (first === 2) screen[row]?.fill(' ')
          else if (!(first > 0)) screen[row]?.fill(' ', column)
          break
        case 'h':
          // The alternate screen starts blank; everything before its switch is gone.
          if (parameters.startsWith('?1049')) screen.forEach((line) => line.fill(' '))
          break
        default:
          break
      }
      continue
    }
    const character = String.fromCodePoint(source.codePointAt(index) ?? 0)
    const line = screen[row]
    index += character.length
    switch (character) {
      case '\r':
        column = 0
        break
      case '\n':
        row = Math.min(row + 1, rows - 1)
        break
      case '\b':
        column = Math.max(column - 1, 0)
        break
      case '\t':
        column = Math.min(column + 1, columns - 1)
        break
      default:
        if (line !== undefined && character >= ' ') {
          line[column] = character
          // A wide grapheme owns two cells; the second is left as the fill it overlays.
          const width = isWide(character) ? 2 : 1
          if (width === 2 && column + 1 < columns) line[column + 1] = ''
          column = Math.min(column + width, columns - 1)
        }
        break
    }
  }
  return screen.map((line) => line.join('').replace(/\s+$/u, '')).join('\n')
}
