import type {
  StorageConversationForkPoint,
  StorageForkConversationErrorData,
  StorageForkConversationRequest,
  StorageModelSettingsValidationErrorData
} from './storage'
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
const MAX_FORK_IDENTIFIER_BYTES = 512 * 4
const MAX_ACTIVE_COMMAND_SESSIONS = 512
const MAX_MODEL_ID_BYTES = 512 * 4

function expectBoundedForkIdentifier(value: unknown, context: string): string {
  const identifier = expectNonEmptyString(value, context)
  if (new TextEncoder().encode(identifier).byteLength > MAX_FORK_IDENTIFIER_BYTES) {
    throw invalidProtocolValue(context, `must not exceed ${MAX_FORK_IDENTIFIER_BYTES} UTF-8 bytes`)
  }
  return identifier
}

function parseStorageConversationForkPoint(value: unknown): StorageConversationForkPoint {
  const context = 'storage fork conversation request.forkPoint'
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    ['assistant_reply', 'provider_transition_boundary'] as const,
    `${context}.kind`
  )

  if (kind === 'assistant_reply') {
    expectOnlyKeys(record, ['kind', 'assistantMessageId'] as const, context)
    return {
      kind,
      assistantMessageId: expectBoundedForkIdentifier(
        record.assistantMessageId,
        `${context}.assistantMessageId`
      )
    }
  }

  expectOnlyKeys(record, ['kind', 'operationId'] as const, context)
  return {
    kind,
    operationId: expectBoundedForkIdentifier(record.operationId, `${context}.operationId`)
  }
}

export function parseStorageForkConversationRequest(
  value: unknown
): StorageForkConversationRequest {
  const context = 'storage fork conversation request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['requestId', 'sourceConversationId', 'forkPoint'] as const, context)

  const requestId = expectBoundedForkIdentifier(record.requestId, `${context}.requestId`)
  const sourceConversationId = expectBoundedForkIdentifier(
    record.sourceConversationId,
    `${context}.sourceConversationId`
  )
  return {
    requestId,
    sourceConversationId,
    forkPoint: parseStorageConversationForkPoint(record.forkPoint)
  }
}

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

export function parseStorageModelSettingsValidationErrorData(
  value: unknown
): StorageModelSettingsValidationErrorData {
  const context = 'storage model settings validation error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['kind', 'code', 'modelId'] as const, context)

  const modelId = expectNonEmptyString(record.modelId, `${context}.modelId`)
  if (new TextEncoder().encode(modelId).byteLength > MAX_MODEL_ID_BYTES) {
    throw invalidProtocolValue(
      `${context}.modelId`,
      `must not exceed ${MAX_MODEL_ID_BYTES} UTF-8 bytes`
    )
  }

  return {
    kind: expectEnum(record.kind, ['model_settings_validation'] as const, `${context}.kind`),
    code: expectEnum(record.code, ['duplicate_model_id'] as const, `${context}.code`),
    modelId
  }
}
