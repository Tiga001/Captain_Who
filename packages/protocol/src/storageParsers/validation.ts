import { invalidProtocolValue, expectString } from '../skills/validation'

const FORBIDDEN_CREDENTIAL_PROJECTION_KEYS = new Set([
  'apiKey',
  'apiToken',
  'apiTokenOverride',
  'credentialRef',
  'credential_ref',
  'tavilyApiKey'
])

export function assertSecretFreeProjection(value: unknown, context: string): void {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertSecretFreeProjection(item, `${context}[${index}]`))
    return
  }
  if (typeof value !== 'object' || value === null) return
  for (const [key, nested] of Object.entries(value)) {
    if (FORBIDDEN_CREDENTIAL_PROJECTION_KEYS.has(key)) {
      throw invalidProtocolValue(context, `forbidden credential field ${key}`)
    }
    assertSecretFreeProjection(nested, `${context}.${key}`)
  }
}

export function expectNullableString(value: unknown, context: string): string | null {
  return value === null ? null : expectString(value, context)
}
