import type { ConversationTurnTraceItem } from './agent'
import {
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'

// Internal audit data. It is deliberately not an AgentEvent or a Renderer timeline action.
export function parseConversationBackendState(
  value: unknown,
  context = 'conversation backend state'
): Extract<ConversationTurnTraceItem, { type: 'backend_state' }> {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['type', 'sequence', 'eventId', 'content', 'createdAt', 'placement'],
    context
  )
  const eventId = expectNonEmptyString(record.eventId, `${context}.eventId`)
  const content = expectString(record.content, `${context}.content`)
  if (
    new TextEncoder().encode(eventId).length > 512 ||
    new TextEncoder().encode(content).length > 16 * 1024
  ) {
    throw invalidProtocolValue(context, 'backend state exceeds its byte limit')
  }
  let payload: unknown
  try {
    payload = JSON.parse(content)
  } catch {
    throw invalidProtocolValue(context, 'content must be a JSON object')
  }
  expectRecord(payload, `${context}.content`)
  return {
    type: expectEnum(record.type, ['backend_state'] as const, `${context}.type`),
    sequence: expectSafeInteger(record.sequence, `${context}.sequence`, 0),
    eventId,
    content,
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
    placement: expectEnum(
      record.placement,
      ['timeline', 'after_message'] as const,
      `${context}.placement`
    )
  }
}
