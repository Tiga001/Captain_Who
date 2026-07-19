export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0
}

export function expectRecord(value: unknown, context: string): Record<string, unknown> {
  if (!isRecord(value)) throw invalidProtocolValue(context, 'expected an object')
  return value
}

export function expectOnlyKeys(
  record: Record<string, unknown>,
  allowedKeys: readonly string[],
  context: string
): void {
  const unexpectedKey = Object.keys(record).find((key) => !allowedKeys.includes(key))
  if (unexpectedKey) {
    throw invalidProtocolValue(context, `unexpected field ${unexpectedKey}`)
  }
}

export function expectArray(value: unknown, context: string): unknown[] {
  if (!Array.isArray(value)) throw invalidProtocolValue(context, 'expected an array')
  return value
}

export function expectNonEmptyString(value: unknown, context: string): string {
  if (!isNonEmptyString(value)) throw invalidProtocolValue(context, 'expected a non-empty string')
  return value
}

export function expectString(value: unknown, context: string): string {
  if (typeof value !== 'string') throw invalidProtocolValue(context, 'expected a string')
  return value
}

export function expectFullGitCommitSha(value: unknown, context: string): string {
  const sha = expectNonEmptyString(value, context)
  if (!/^[0-9a-f]{40}$/.test(sha)) {
    throw invalidProtocolValue(context, 'expected a lower-case 40-character hexadecimal SHA')
  }
  return sha
}

export function expectCanonicalNonNilUuid(value: unknown, context: string): string {
  const uuid = expectNonEmptyString(value, context)
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(uuid) ||
    uuid === '00000000-0000-0000-0000-000000000000'
  ) {
    throw invalidProtocolValue(context, 'expected a canonical non-nil lower-case UUID')
  }
  return uuid
}

export function optionalNonEmptyString(value: unknown, context: string): string | undefined {
  if (value === undefined) return undefined
  return expectNonEmptyString(value, context)
}

export function expectBoolean(value: unknown, context: string): boolean {
  if (typeof value !== 'boolean') throw invalidProtocolValue(context, 'expected a boolean')
  return value
}

export function expectSafeInteger(value: unknown, context: string, minimum: number): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum) {
    throw invalidProtocolValue(
      context,
      `expected a safe integer greater than or equal to ${minimum}`
    )
  }
  return value
}

export function expectEnum<const T extends readonly string[]>(
  value: unknown,
  values: T,
  context: string
): T[number] {
  if (typeof value !== 'string' || !(values as readonly string[]).includes(value)) {
    throw invalidProtocolValue(context, `unexpected value ${String(value)}`)
  }
  return value as T[number]
}

export function expectSchemaVersion(
  record: Record<string, unknown>,
  expected: number,
  context: string
): void {
  if (record.schemaVersion !== expected) {
    throw invalidProtocolValue(
      context,
      `unsupported schema version ${String(record.schemaVersion)}`
    )
  }
}

export function invalidProtocolValue(context: string, reason: string): Error {
  return new Error(`Invalid ${context}: ${reason}`)
}
