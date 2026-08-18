import type { ChatAgentRunView } from '../chat/chatTypes'

export const MAX_STORED_RUN_ITEMS = 10_000

const AGENT_INTERRUPTION_REASONS = new Set<NonNullable<ChatAgentRunView['interruption']>['reason']>(
  [
    'service_connection_failed',
    'service_unavailable',
    'authentication_failed',
    'quota_exhausted',
    'context_limit_exceeded',
    'request_rejected',
    'response_invalid',
    'request_failed'
  ]
)

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function hasOwn(record: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(record, key)
}

export function isAgentInterruption(
  value: unknown
): value is NonNullable<ChatAgentRunView['interruption']> {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['reason']) &&
    typeof value.reason === 'string' &&
    AGENT_INTERRUPTION_REASONS.has(
      value.reason as NonNullable<ChatAgentRunView['interruption']>['reason']
    )
  )
}

export function hasExactKeys(
  record: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[] = []
): boolean {
  const allowed = new Set([...required, ...optional])
  return (
    required.every((key) => hasOwn(record, key)) &&
    Object.keys(record).every((key) => allowed.has(key))
  )
}

export function isBoundedString(
  value: unknown,
  maximum = 4096,
  allowEmpty = false
): value is string {
  return (
    typeof value === 'string' &&
    (allowEmpty || value.length > 0) &&
    Array.from(value).length <= maximum
  )
}

export function isOptionalBoundedString(
  record: Record<string, unknown>,
  key: string,
  maximum = 4096,
  allowEmpty = false
): boolean {
  return !hasOwn(record, key) || isBoundedString(record[key], maximum, allowEmpty)
}

export function isSafeInteger(value: unknown, minimum = 0): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum
}

export function isOptionalSafeInteger(
  record: Record<string, unknown>,
  key: string,
  minimum = 0
): boolean {
  return !hasOwn(record, key) || isSafeInteger(record[key], minimum)
}

export function isRecordArray(
  value: unknown,
  validate: (record: Record<string, unknown>) => boolean
): boolean {
  return (
    Array.isArray(value) &&
    value.length <= MAX_STORED_RUN_ITEMS &&
    value.every((item) => isRecord(item) && validate(item))
  )
}

export function isStringArray(value: unknown, maximumLength = 4096): value is string[] {
  return (
    Array.isArray(value) &&
    value.length <= MAX_STORED_RUN_ITEMS &&
    value.every((entry) => isBoundedString(entry, maximumLength, true))
  )
}

export function isNullableBoundedString(
  value: unknown,
  maximum = 4096,
  allowEmpty = false
): value is string | null {
  return value === null || isBoundedString(value, maximum, allowEmpty)
}

export function isNullableSafeInteger(value: unknown, minimum = 0): value is number | null {
  return value === null || isSafeInteger(value, minimum)
}

export function hasUniqueStrings(values: readonly string[]): boolean {
  return new Set(values).size === values.length
}

export function isApprovalStatus(value: unknown): boolean {
  return (
    value === 'not_required' || value === 'required' || value === 'approved' || value === 'rejected'
  )
}
