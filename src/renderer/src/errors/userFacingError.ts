import type { TranslationKey } from '../config/frontendTranslations'
import type { Translate } from '../config/translationFormat'
import type { ChatAgentInterruptionReason } from '../features/chat/chatTypes'

type UnknownRecord = Record<string, unknown>

export interface UserFacingErrorMapping {
  readonly codes?: Readonly<Record<string, TranslationKey>>
  readonly types?: Readonly<Record<string, TranslationKey>>
}

function isRecord(value: unknown): value is UnknownRecord {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function normalizeDiscriminator(value: string): string {
  return value.replace(/[^a-z0-9]/gi, '').toLowerCase()
}

function collectStructuredErrorFields(error: unknown): {
  codes: string[]
  types: string[]
} {
  const records: UnknownRecord[] = []
  if (isRecord(error)) records.push(error)

  const rootData = records[0]?.data
  if (isRecord(rootData)) {
    records.push(rootData)
    if (isRecord(rootData.error)) records.push(rootData.error)
  }

  const codes: string[] = []
  const types: string[] = []
  for (const record of records) {
    if (typeof record.code === 'string') codes.push(record.code)
    if (typeof record.type === 'string') types.push(record.type)
  }

  return { codes, types }
}

function resolveMappedKey(
  values: readonly string[],
  mapping: Readonly<Record<string, TranslationKey>> | undefined
): TranslationKey | undefined {
  if (!mapping) return undefined

  for (const value of values) {
    const direct = mapping[value]
    if (direct) return direct

    const normalized = normalizeDiscriminator(value)
    const normalizedMatch = Object.entries(mapping).find(
      ([candidate]) => normalizeDiscriminator(candidate) === normalized
    )
    if (normalizedMatch) return normalizedMatch[1]
  }

  return undefined
}

/**
 * Converts structured host errors to translation keys. Raw exception messages deliberately stay
 * outside this boundary so provider and implementation details cannot leak into product UI.
 */
export function getUserFacingErrorKey(
  error: unknown,
  fallbackKey: TranslationKey,
  mapping: UserFacingErrorMapping = {}
): TranslationKey {
  const fields = collectStructuredErrorFields(error)
  return (
    resolveMappedKey(fields.codes, mapping.codes) ??
    resolveMappedKey(fields.types, mapping.types) ??
    fallbackKey
  )
}

export function getUserFacingErrorMessage(
  error: unknown,
  t: Translate,
  fallbackKey: TranslationKey,
  mapping?: UserFacingErrorMapping
): string {
  return t(getUserFacingErrorKey(error, fallbackKey, mapping))
}

const AGENT_INTERRUPTION_BY_DISCRIMINATOR: Readonly<Record<string, ChatAgentInterruptionReason>> = {
  networkunavailable: 'service_connection_failed',
  networkerror: 'service_connection_failed',
  connectionfailed: 'service_connection_failed',
  serviceconnectionfailed: 'service_connection_failed',
  serviceunavailable: 'service_unavailable',
  providerunavailable: 'service_unavailable',
  authenticationfailed: 'authentication_failed',
  unauthorized: 'authentication_failed',
  invalidapikey: 'authentication_failed',
  quotaexhausted: 'quota_exhausted',
  ratelimitexceeded: 'quota_exhausted',
  contextlimitexceeded: 'context_limit_exceeded',
  contextlengthexceeded: 'context_limit_exceeded',
  requestrejected: 'request_rejected',
  invalidstreamtoolarguments: 'response_invalid',
  invalidresponse: 'response_invalid',
  responseinvalid: 'response_invalid'
}

export function getAgentInterruptionReason(error: unknown): ChatAgentInterruptionReason {
  const fields = collectStructuredErrorFields(error)
  for (const value of [...fields.codes, ...fields.types]) {
    const reason = AGENT_INTERRUPTION_BY_DISCRIMINATOR[normalizeDiscriminator(value)]
    if (reason) return reason
  }
  return 'request_failed'
}
