import { readFileSync } from 'node:fs'
import { type AST, parseTOML } from 'toml-eslint-parser'
import { ReleaseError } from './release-error.ts'

export class TomlTimestamp {
  constructor(
    readonly kind: AST.TOMLDateTimeValue['kind'],
    readonly text: string,
  ) {}
}

export type TomlValue = string | number | bigint | boolean | TomlTimestamp | TomlValue[] | TomlTable
export interface TomlTable {
  [key: string]: TomlValue
}
export interface TomlStringLiteral {
  readonly path: readonly (string | number)[]
  readonly range: readonly [number, number]
}
export interface TomlDocument {
  readonly values: TomlTable
  readonly strings: readonly TomlStringLiteral[]
}

export function isTomlTable(value: TomlValue | undefined): value is TomlTable {
  return typeof value === 'object' && !Array.isArray(value) && !(value instanceof TomlTimestamp)
}

export function tomlTable(value: TomlValue | undefined, description: string): TomlTable {
  if (!isTomlTable(value)) throw new ReleaseError(`${description} must be a TOML table`)
  return value
}

export function tomlString(value: TomlValue | undefined, description: string): string {
  if (typeof value !== 'string') throw new ReleaseError(`${description} must be a string`)
  return value
}

function keyParts(key: AST.TOMLKey): string[] {
  return key.keys.map((part) => (part.type === 'TOMLBare' ? part.name : part.value))
}

/** Preserve integers and timestamp precision rather than coercing TOML through JSON. */
export function parseToml(source: string): TomlDocument {
  const ast = parseTOML(source, { tomlVersion: '1.0' })
  const values: TomlTable = Object.create(null)
  const strings: TomlStringLiteral[] = []

  function assign(
    container: TomlTable | TomlValue[],
    path: readonly (string | number)[],
    value?: TomlValue,
  ): TomlTable | TomlValue[] {
    let current = container
    for (let index = 0; index < path.length; index++) {
      const key = path[index]
      if (key === undefined) throw new ReleaseError('missing TOML key')
      const last = index === path.length - 1
      let next: TomlValue | undefined
      if (Array.isArray(current)) {
        if (typeof key !== 'number') throw new ReleaseError('invalid TOML array index')
        if (last && value !== undefined) {
          current[key] = value
          return current
        }
        next = current[key]
        if (next === undefined) {
          next = typeof path[index + 1] === 'number' ? [] : (Object.create(null) as TomlTable)
          current[key] = next
        }
      } else {
        if (typeof key !== 'string') throw new ReleaseError('invalid TOML table key')
        if (last && value !== undefined) {
          current[key] = value
          return current
        }
        next = current[key]
        if (next === undefined) {
          next = typeof path[index + 1] === 'number' ? [] : (Object.create(null) as TomlTable)
          current[key] = next
        }
      }
      if (!Array.isArray(next) && !isTomlTable(next)) throw new ReleaseError('TOML key traverses a scalar')
      current = next
    }
    return current
  }

  function decode(node: AST.TOMLContentNode, path: readonly (string | number)[]): TomlValue {
    if (node.type === 'TOMLArray') {
      return node.elements.map((element, index) => decode(element, [...path, index]))
    }
    if (node.type === 'TOMLInlineTable') {
      const table: TomlTable = Object.create(null)
      for (const declaration of node.body) {
        const keys = keyParts(declaration.key)
        assign(table, keys, decode(declaration.value, [...path, ...keys]))
      }
      return table
    }
    switch (node.kind) {
      case 'integer':
        return node.bigint
      case 'string':
        strings.push({ path, range: node.range })
        return node.value
      case 'float':
      case 'boolean':
        return node.value
      default:
        return new TomlTimestamp(node.kind, node.datetime)
    }
  }

  for (const node of ast.body[0].body) {
    if (node.type === 'TOMLKeyValue') {
      const keys = keyParts(node.key)
      assign(values, keys, decode(node.value, keys))
    } else {
      assign(values, node.resolvedKey)
      for (const declaration of node.body) {
        const keys = [...node.resolvedKey, ...keyParts(declaration.key)]
        assign(values, keys, decode(declaration.value, keys))
      }
    }
  }
  return { values, strings }
}

export function readToml(path: string): TomlTable {
  return parseToml(new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(readFileSync(path)))
    .values
}
