import {
  expectArray,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'

export const HUMAN_INTERACTION_GET_ATTENTION_METHOD = 'humanInteraction.getAttention'

/** Sparse, atomic snapshot. Questions/answers and approval payloads stay in their detail APIs. */
export interface HumanInteractionAttentionRequest {
  requestId: string
  conversationId: string
  sequence: number
  revision: number
}
export interface HumanInteractionAttentionSnapshot {
  requestSequence: number
  requests: HumanInteractionAttentionRequest[]
  approvalConversationIds: string[]
}

function id(value: unknown): string {
  const result = expectString(value, 'attention identity')
  if (
    !result ||
    result.trim() !== result ||
    new TextEncoder().encode(result).length > 256 ||
    Array.from(result).some((character) => {
      const code = character.charCodeAt(0)
      return code < 32 || (code >= 127 && code <= 159)
    })
  ) {
    throw invalidProtocolValue('attention identity', 'invalid identifier')
  }
  return result
}

export function parseHumanInteractionAttentionSnapshot(
  value: unknown
): HumanInteractionAttentionSnapshot {
  const context = 'human interaction attention snapshot'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['requestSequence', 'requests', 'approvalConversationIds'], context)
  const requestSequence = expectSafeInteger(record.requestSequence, context, 0)
  const ids = new Set<string>(),
    sequences = new Set<number>()
  const requests = expectArray(record.requests, context).map((value) => {
    const item = expectRecord(value, context)
    expectOnlyKeys(item, ['requestId', 'conversationId', 'sequence', 'revision'], context)
    const result = {
      requestId: id(item.requestId),
      conversationId: id(item.conversationId),
      sequence: expectSafeInteger(item.sequence, context, 1),
      revision: expectSafeInteger(item.revision, context, 0)
    }
    if (
      result.sequence > requestSequence ||
      ids.has(result.requestId) ||
      sequences.has(result.sequence)
    ) {
      throw invalidProtocolValue(context, 'invalid snapshot watermark or duplicate identity')
    }
    ids.add(result.requestId)
    sequences.add(result.sequence)
    return result
  })
  const approvalConversationIds = expectArray(record.approvalConversationIds, context).map(id)
  if (new Set(approvalConversationIds).size !== approvalConversationIds.length) {
    throw invalidProtocolValue(context, 'duplicate approval conversation')
  }
  return { requestSequence, requests, approvalConversationIds }
}
