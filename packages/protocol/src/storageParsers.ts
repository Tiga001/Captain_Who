import type { StorageForkConversationErrorData } from './storage'
import {
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from './skills/validation'

// Core accepts at most 512 Unicode scalar values; four bytes each covers the same identifier set.
const MAX_CONVERSATION_ID_BYTES = 512 * 4
const MAX_ACTIVE_COMMAND_SESSIONS = 512

export function parseStorageForkConversationErrorData(
  value: unknown
): StorageForkConversationErrorData {
  const context = 'storage fork conversation error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'code', 'conversationId', 'activeSessionCount'] as const, context)

  const conversationId = expectNonEmptyString(record.conversationId, `${context}.conversationId`)
  if (new TextEncoder().encode(conversationId).byteLength > MAX_CONVERSATION_ID_BYTES) {
    throw invalidProtocolValue(
      `${context}.conversationId`,
      `must not exceed ${MAX_CONVERSATION_ID_BYTES} UTF-8 bytes`
    )
  }

  const activeSessionCount = expectSafeInteger(
    record.activeSessionCount,
    `${context}.activeSessionCount`,
    1
  )
  if (activeSessionCount > MAX_ACTIVE_COMMAND_SESSIONS) {
    throw invalidProtocolValue(
      `${context}.activeSessionCount`,
      `must not exceed ${MAX_ACTIVE_COMMAND_SESSIONS}`
    )
  }

  return {
    type: expectEnum(record.type, ['conversation_fork'] as const, `${context}.type`),
    code: expectEnum(record.code, ['active_command_session'] as const, `${context}.code`),
    conversationId,
    activeSessionCount
  }
}
