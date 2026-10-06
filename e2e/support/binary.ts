import { constants } from 'node:fs'
import { access, stat } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const REPOSITORY_ROOT = resolve(__dirname, '../..')

export async function findBinary(): Promise<string> {
  const configured = process.env.TOLA_E2E_BIN
  if (configured === '') throw new Error('TOLA_E2E_BIN must name an executable')
  const binary = configured === undefined
    ? join(REPOSITORY_ROOT, 'target/debug', process.platform === 'win32' ? 'tola.exe' : 'tola')
    : resolve(configured)
  try {
    if (!(await stat(binary)).isFile()) throw new Error('path is not a file')
    await access(binary, constants.X_OK)
  } catch (error) {
    const instruction = configured === undefined
      ? 'run just e2e or build it with cargo build --locked -p tola'
      : 'set TOLA_E2E_BIN to an existing executable'
    throw new Error(`Tola executable is unavailable at ${binary}; ${instruction}`, { cause: error })
  }
  return binary
}
