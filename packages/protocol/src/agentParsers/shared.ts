import { AGENT_FILE_CHANGE_SCHEMA_VERSION } from '../agent'
import { expectSafeInteger, expectString, invalidProtocolValue } from '../skills/validation'
export const DIGEST_PATTERN = /^[0-9a-f]{64}$/
export const UUID_V4_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
export const SAFE_CODE_PATTERN = /^[a-zA-Z0-9_.-]{1,128}$/
export const CANONICAL_PROVIDER_CODE_PATTERN = /^[a-z0-9][a-z0-9_.-]{0,127}$/
export const MODEL_TOOL_CALL_ID_PATTERN = /^tc1_[a-zA-Z0-9_-]{43}$/
export const MAX_RENDERER_DATE_UNIX_SECONDS = 253_402_300_799
export const MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES = 1024 * 1024
export const MAX_RENDERER_SAFE_AGENT_EVENT_BYTES = 16 * 1024 * 1024
export const MAX_RENDERER_SAFE_PROPOSED_ACTIONS = 1024

export function assertRendererSafeJson(
  value: unknown,
  context: string,
  maximumBytes = MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
): void {
  let encoded: string | undefined
  try {
    encoded = JSON.stringify(value)
  } catch {
    throw invalidProtocolValue(context, 'must be JSON encodable')
  }
  if (encoded === undefined || new TextEncoder().encode(encoded).byteLength > maximumBytes) {
    throw invalidProtocolValue(context, `exceeded the Renderer-safe ${maximumBytes} byte limit`)
  }
}

export function expectBoundedArray(
  value: unknown,
  context: string,
  maximumItems: number
): unknown[] {
  if (!Array.isArray(value) || value.length > maximumItems) {
    throw invalidProtocolValue(context, `expected an array with at most ${maximumItems} items`)
  }
  return value
}

export function expectFileChangeSchemaVersion(value: unknown, context: string): 1 {
  const version = expectSafeInteger(value, context, 0)
  if (version !== AGENT_FILE_CHANGE_SCHEMA_VERSION) {
    throw invalidProtocolValue(context, 'unsupported FileChange schema version')
  }
  return AGENT_FILE_CHANGE_SCHEMA_VERSION
}

export function expectSignedSafeInteger(value: unknown, context: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) {
    throw invalidProtocolValue(context, 'expected a safe integer')
  }
  return value
}

export function expectExactSchemaVersion(
  value: unknown,
  expected: number,
  context: string
): number {
  const version = expectSafeInteger(value, context, 1)
  if (version !== expected) {
    throw invalidProtocolValue(context, `expected schema version ${expected}`)
  }
  return version
}

export function parseStringArray(
  value: unknown,
  context: string,
  maximumItems: number,
  maximumItemBytes: number
): string[] {
  return expectBoundedArray(value, context, maximumItems).map((entry, index) =>
    expectBoundedString(entry, `${context}[${index}]`, maximumItemBytes)
  )
}

export function expectDisplayText(value: unknown, context: string, maxBytes: number): string {
  const result = expectBoundedString(value, context, maxBytes)
  if (hasDisallowedDisplayControl(result)) {
    throw invalidProtocolValue(context, 'contains disallowed control characters')
  }
  return result
}

export function expectServerDisplayName(value: unknown, context: string): string {
  const result = expectBoundedString(value, context, 256)
  if (result.length === 0 || result.trim() !== result || hasAnyControl(result)) {
    throw invalidProtocolValue(context, 'expected a trimmed display name without controls')
  }
  return result
}

export function expectOpaqueRunId(value: unknown, context: string): string {
  const result = expectBoundedString(value, context, 2048)
  if (result.length === 0 || result.trim() !== result || hasAnyControl(result)) {
    throw invalidProtocolValue(context, 'expected a trimmed opaque run identity without controls')
  }
  return result
}

export function expectModelToolCallId(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!MODEL_TOOL_CALL_ID_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a canonical application-owned Tool Call id')
  }
  return result
}

export function hasDisallowedDisplayControl(value: string): boolean {
  return /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value)
}

export function hasAnyControl(value: string): boolean {
  return /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value)
}

export function expectBoundedNonEmptyString(
  value: unknown,
  context: string,
  maxBytes: number
): string {
  const result = expectBoundedString(value, context, maxBytes)
  if (result.length === 0) {
    throw invalidProtocolValue(context, 'must not be empty')
  }
  return result
}

export function expectBoundedString(value: unknown, context: string, maxBytes: number): string {
  const result = expectString(value, context)
  if (new TextEncoder().encode(result).byteLength > maxBytes) {
    throw invalidProtocolValue(context, `exceeded ${maxBytes} UTF-8 bytes`)
  }
  return result
}

export function expectSafeCode(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!SAFE_CODE_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a bounded safe code')
  }
  return result
}

export function parseNullableHttpOrigin(value: unknown, context: string): string | null {
  if (value === null) return null
  const origin = expectDisplayText(value, context, 2048)
  if (origin === 'file://') return origin
  let parsed: URL
  try {
    parsed = new URL(origin)
  } catch {
    throw invalidProtocolValue(context, 'must be an HTTP(S) or file origin')
  }
  if (
    !['http:', 'https:'].includes(parsed.protocol) ||
    parsed.origin !== origin ||
    parsed.username !== '' ||
    parsed.password !== '' ||
    parsed.pathname !== '/' ||
    parsed.search !== '' ||
    parsed.hash !== ''
  ) {
    throw invalidProtocolValue(context, 'must be a canonical HTTP(S) or file origin')
  }
  return origin
}

export function expectExactString(value: unknown, expected: string, context: string): string {
  if (value !== expected) {
    throw invalidProtocolValue(context, `expected ${expected}`)
  }
  return expected
}

export function parseOptionalNullableString(value: unknown, context: string): string | null {
  if (value === undefined || value === null) return null
  return expectBoundedNonEmptyString(value, context, 256)
}

export function expectUuid(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result) ||
    result === '00000000-0000-0000-0000-000000000000'
  ) {
    throw invalidProtocolValue(context, 'expected a canonical non-nil lower-case UUID')
  }
  return result
}

export function expectUuidV4(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!UUID_V4_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a canonical lower-case UUIDv4')
  }
  return result
}

export function expectDigest(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!DIGEST_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a lower-case SHA-256 digest')
  }
  return result
}

export function expectVersionedSha256Digest(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!/^sha256:[0-9a-f]{64}$/.test(result)) {
    throw invalidProtocolValue(context, 'expected a sha256:-prefixed lower-case digest')
  }
  return result
}

export function expectBuiltinCapabilityStableId(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    result.length === 0 ||
    new TextEncoder().encode(result).byteLength > 128 ||
    !/^[a-z](?:[a-z0-9._-]*[a-z0-9])?$/.test(result) ||
    result.includes('..')
  ) {
    throw invalidProtocolValue(context, 'expected a stable lower-case built-in capability id')
  }
  return result
}

export function expectBuiltinCapabilityModelName(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    result.length === 0 ||
    new TextEncoder().encode(result).byteLength > 64 ||
    !/^[A-Za-z0-9_-]+$/.test(result)
  ) {
    throw invalidProtocolValue(context, 'expected a bounded Provider-visible Tool name')
  }
  return result
}

export function expectBuiltinCapabilityPackageIdentity(
  value: unknown,
  context: string,
  maxBytes: number
): string {
  const result = expectBoundedNonEmptyString(value, context, maxBytes)
  if (result.trim() !== result || hasAnyControl(result)) {
    throw invalidProtocolValue(context, 'expected a trimmed package identity without controls')
  }
  return result
}
