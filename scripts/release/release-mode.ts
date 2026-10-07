import { ReleaseError } from './release-error.ts'

export type ReleaseMode = 'create' | 'update-preserve-notes' | 'update-regenerate-notes'

export function parseReleaseMode(value: string | undefined): ReleaseMode {
  if (value === undefined) return 'create'
  if (value === 'create' || value === 'update-preserve-notes' || value === 'update-regenerate-notes') {
    return value
  }
  throw new ReleaseError(`invalid release mode: ${value}`)
}
