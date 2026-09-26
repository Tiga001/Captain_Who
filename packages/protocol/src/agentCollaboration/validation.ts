import {
  AGENT_COLLABORATION_SCHEMA_VERSION,
  AGENT_COLLABORATION_EVENT_SCHEMA_VERSION,
  AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION
} from './constants'

const MAX_ID_BYTES = 512

export const MAX_TEXT_BYTES = 65_536

type JsonRecord = Record<string, unknown>

export function record(value: unknown, context: string): JsonRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`Invalid ${context}`)
  }
  return value as JsonRecord
}

export function exact(value: JsonRecord, keys: readonly string[], context: string): void {
  const allowed = new Set(keys)
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) throw new Error(`Invalid ${context}.${key}`)
  }
  for (const key of keys) {
    if (!(key in value)) throw new Error(`Missing ${context}.${key}`)
  }
}

export function text(value: unknown, context: string, maximum = MAX_ID_BYTES): string {
  if (typeof value !== 'string' || value.length === 0 || value.trim() !== value) {
    throw new Error(`Invalid ${context}`)
  }
  if (new TextEncoder().encode(value).byteLength > maximum) throw new Error(`Invalid ${context}`)
  return value
}

export function nullableText(value: unknown, context: string): string | null {
  return value === null ? null : text(value, context)
}

export function utf8LessThan(left: string, right: string): boolean {
  const leftBytes = new TextEncoder().encode(left)
  const rightBytes = new TextEncoder().encode(right)
  const sharedLength = Math.min(leftBytes.length, rightBytes.length)
  for (let index = 0; index < sharedLength; index += 1) {
    if (leftBytes[index] !== rightBytes[index]) return leftBytes[index]! < rightBytes[index]!
  }
  return leftBytes.length < rightBytes.length
}

export function boundedOptionalString(
  value: unknown,
  context: string,
  maximum: number,
  requireJson = false
): string | null {
  if (value === null) return null
  if (typeof value !== 'string' || new TextEncoder().encode(value).byteLength > maximum) {
    throw new Error(`Invalid ${context}`)
  }
  if (requireJson) {
    try {
      JSON.parse(value)
    } catch {
      throw new Error(`Invalid ${context}`)
    }
  }
  return value
}

export function integer(
  value: unknown,
  context: string,
  minimum = 0,
  maximum = Number.MAX_SAFE_INTEGER
) {
  if (!Number.isSafeInteger(value) || (value as number) < minimum || (value as number) > maximum) {
    throw new Error(`Invalid ${context}`)
  }
  return value as number
}

export function bool(value: unknown, context: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`Invalid ${context}`)
  return value
}

export function oneOf<const T extends readonly string[]>(
  value: unknown,
  choices: T,
  context: string
): T[number] {
  if (typeof value !== 'string' || !(choices as readonly string[]).includes(value)) {
    throw new Error(`Invalid ${context}`)
  }
  return value as T[number]
}

export function schema(value: unknown, context: string): typeof AGENT_COLLABORATION_SCHEMA_VERSION {
  if (value !== AGENT_COLLABORATION_SCHEMA_VERSION)
    throw new Error(`Invalid ${context}.schemaVersion`)
  return AGENT_COLLABORATION_SCHEMA_VERSION
}

export function eventSchema(
  value: unknown,
  context: string
): typeof AGENT_COLLABORATION_EVENT_SCHEMA_VERSION {
  if (value !== AGENT_COLLABORATION_EVENT_SCHEMA_VERSION)
    throw new Error(`Invalid ${context}.schemaVersion`)
  return AGENT_COLLABORATION_EVENT_SCHEMA_VERSION
}

export function activitySchema(
  value: unknown,
  context: string
): typeof AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION {
  if (value !== AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION)
    throw new Error(`Invalid ${context}.schemaVersion`)
  return AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION
}
